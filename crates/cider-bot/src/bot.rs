use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
};

use tokio::sync::{Mutex, RwLock};

use crate::{
    command::{self, Command, CommandArg},
    config::Config,
    error::{BotError, StoreError},
    napcat::NapcatClient,
    protocol::{Event, Id, MessageSegment, QuickOperation},
    store::{FeatureKey, FeatureManifest, FeatureRecord, FeatureStore},
};

const DICE: &str = "dice";
const TITLE: &str = "title";
const SESSION_MODE: &str = "session_mode";

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
enum DiceSessionMode {
    Strict,
    #[default]
    Common,
}

impl DiceSessionMode {
    fn parse(value: &str) -> Option<Self> {
        match value {
            "strict" => Some(Self::Strict),
            "common" => Some(Self::Common),
            _ => None,
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::Strict => "strict",
            Self::Common => "common",
        }
    }

    fn from_manifest(manifest: &FeatureManifest) -> Self {
        match manifest.get(SESSION_MODE) {
            Some(toml::Value::String(value)) => Self::parse(value).unwrap_or_else(|| {
                tracing::warn!(
                    session_mode = value,
                    "invalid dice session mode; using the default"
                );
                Self::default()
            }),
            None => Self::Common,
            Some(value) => {
                tracing::warn!(
                    session_mode = %value,
                    "invalid dice session mode; using the default"
                );
                Self::Common
            }
        }
    }
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
struct GroupKey {
    self_id: String,
    group_id: String,
}

impl GroupKey {
    fn new(self_id: &Id, group_id: &Id) -> Self {
        Self {
            self_id: self_id.as_str().into(),
            group_id: group_id.as_str().into(),
        }
    }

    fn feature(&self, feature_name: &str) -> FeatureKey {
        FeatureKey {
            self_id: self.self_id.clone(),
            group_id: self.group_id.clone(),
            feature_name: feature_name.into(),
        }
    }
}

#[derive(Debug, Default)]
struct DiceSession {
    users: HashSet<Id>,
    rolls: Vec<(Id, u8)>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Role {
    Owner,
    Admin,
    Member,
}

impl Role {
    fn parse(value: Option<&str>) -> Self {
        match value {
            Some("owner") => Self::Owner,
            Some("admin") => Self::Admin,
            _ => Self::Member,
        }
    }

    fn privileged(self) -> bool {
        matches!(self, Self::Owner | Self::Admin)
    }
}

pub struct Bot {
    owners: HashSet<String>,
    store: FeatureStore,
    features: RwLock<HashMap<FeatureKey, FeatureRecord>>,
    sessions: Mutex<HashMap<GroupKey, DiceSession>>,
    group_locks: Mutex<HashMap<GroupKey, Arc<Mutex<()>>>>,
    pub napcat: NapcatClient,
}

impl Bot {
    pub fn new(
        config: &Config,
        store: FeatureStore,
        features: HashMap<FeatureKey, FeatureRecord>,
    ) -> Self {
        let napcat = NapcatClient::new(
            &config.napcat_base_url,
            &config.api_token,
            config.napcat_timeout_ms,
        )
        .expect("validated NapCat client configuration");
        Self {
            owners: config.owners.clone(),
            store,
            features: RwLock::new(features),
            sessions: Mutex::new(HashMap::new()),
            group_locks: Mutex::new(HashMap::new()),
            napcat,
        }
    }

    pub async fn process(&self, event: Event) -> Result<Option<QuickOperation>, BotError> {
        if event.post_type != "message"
            || event.message_type.as_deref() != Some("group")
            || event.sub_type.as_deref() != Some("normal")
        {
            return Ok(None);
        }
        let (Some(user_id), Some(group_id)) = (event.user_id.as_ref(), event.group_id.as_ref())
        else {
            return Ok(None);
        };
        if user_id == &event.self_id {
            return Ok(None);
        }

        let group = GroupKey::new(&event.self_id, group_id);

        if let Some(command) = command::parse(&event.message) {
            let role = Role::parse(
                event
                    .sender
                    .as_ref()
                    .and_then(|sender| sender.role.as_deref()),
            );
            if matches!(command.name.as_str(), "enable" | "disable")
                && only_feature(&command.args, TITLE)
            {
                return self
                    .handle_title_toggle(group, &event.self_id, user_id, role, &command.name)
                    .await;
            }
            if command.name == TITLE {
                return self.handle_title(group, user_id, role, &command.args).await;
            }

            let lock = self.group_lock(&group).await;
            let _guard = lock.lock().await;
            return self.handle_command(group, user_id, role, command).await;
        }

        let lock = self.group_lock(&group).await;
        let _guard = lock.lock().await;

        let Some(result) = event.message.iter().find_map(MessageSegment::dice_result) else {
            return Ok(None);
        };
        if !self.is_feature_enabled(&group, DICE).await {
            return Ok(None);
        }
        let mut sessions = self.sessions.lock().await;
        if let Some(session) = sessions.get_mut(&group)
            && session.users.insert(user_id.clone())
        {
            session.rolls.push((user_id.clone(), result));
        }
        Ok(None)
    }

    pub async fn ready(&self) -> bool {
        self.store.ready().await
    }

    async fn handle_command(
        &self,
        group: GroupKey,
        user_id: &Id,
        role: Role,
        command: Command,
    ) -> Result<Option<QuickOperation>, BotError> {
        let response = match command.name.as_str() {
            "enable" => {
                if !only_feature(&command.args, DICE) {
                    text("Usage: .enable dice|title")
                } else if !self.can_administer(user_id, role) {
                    text("Permission denied.")
                } else if self.is_feature_enabled(&group, DICE).await {
                    text("Dice statistics is already enabled.")
                } else {
                    let feature = group.feature(DICE);
                    let record = self.store.enable(&feature, user_id.as_str()).await?;
                    self.features.write().await.insert(feature, record);
                    text("Dice statistics enabled.")
                }
            }
            "disable" => {
                if !only_feature(&command.args, DICE) {
                    text("Usage: .disable dice|title")
                } else if !self.can_administer(user_id, role) {
                    text("Permission denied.")
                } else if !self.is_feature_enabled(&group, DICE).await {
                    text("Dice statistics is already disabled.")
                } else {
                    let feature = group.feature(DICE);
                    if let Some(record) = self.store.disable(&feature).await? {
                        self.features.write().await.insert(feature, record);
                    }
                    self.sessions.lock().await.remove(&group);
                    text("Dice statistics disabled.")
                }
            }
            "dice" => {
                if !command.args.is_empty() {
                    text("Usage: .dice")
                } else if !self.is_feature_enabled(&group, DICE).await {
                    text("Dice statistics is disabled in this group.")
                } else {
                    let session_mode = self.dice_session_mode(&group).await;
                    let mut sessions = self.sessions.lock().await;
                    match sessions.entry(group.clone()) {
                        std::collections::hash_map::Entry::Vacant(entry) => {
                            entry.insert(DiceSession::default());
                            text("Dice session started.")
                        }
                        std::collections::hash_map::Entry::Occupied(mut entry) => {
                            if session_mode == DiceSessionMode::Strict {
                                text("A dice session is already active.")
                            } else {
                                let previous = entry.insert(DiceSession::default());
                                QuickOperation::reply(statistics_message(&previous.rolls))
                            }
                        }
                    }
                }
            }
            "ecid" => {
                if !command.args.is_empty() {
                    text("Usage: .ecid")
                } else if !self.is_feature_enabled(&group, DICE).await {
                    text("Dice statistics is disabled in this group.")
                } else if let Some(session) = self.sessions.lock().await.remove(&group) {
                    QuickOperation::reply(statistics_message(&session.rolls))
                } else {
                    text("No dice session is active.")
                }
            }
            "fset" => {
                let Some((feature, key, value)) = fset_args(&command.args) else {
                    return Ok(Some(text("Usage: .fset feature key value")));
                };
                if !self.can_administer(user_id, role) {
                    text("Permission denied.")
                } else {
                    self.set_feature_manifest(&group, feature, key, value)
                        .await?
                }
            }
            "fget" => {
                let Some((feature, key)) = fget_args(&command.args) else {
                    return Ok(Some(text("Usage: .fget feature [key]")));
                };
                self.get_feature_manifest(&group, feature, key).await?
            }
            _ => return Ok(None),
        };
        Ok(Some(response))
    }

    async fn handle_title_toggle(
        &self,
        group: GroupKey,
        self_id: &Id,
        user_id: &Id,
        role: Role,
        operation: &str,
    ) -> Result<Option<QuickOperation>, BotError> {
        if !self.can_administer(user_id, role) {
            return Ok(Some(text("Permission denied.")));
        }

        let group_id = Id::new(group.group_id.clone()).expect("group key contains a valid ID");
        if self.napcat.group_member_role(&group_id, self_id).await? != "owner" {
            return Ok(Some(text(
                "Title feature requires the bot to be the group owner.",
            )));
        }

        let lock = self.group_lock(&group).await;
        let _guard = lock.lock().await;
        let feature = group.feature(TITLE);
        let enabled = self.is_feature_enabled(&group, TITLE).await;
        let response = match operation {
            "enable" if enabled => text("Title is already enabled."),
            "enable" => {
                let record = self.store.enable(&feature, user_id.as_str()).await?;
                self.features.write().await.insert(feature, record);
                text("Title enabled.")
            }
            "disable" if !enabled => text("Title is already disabled."),
            "disable" => {
                if let Some(record) = self.store.disable(&feature).await? {
                    self.features.write().await.insert(feature, record);
                }
                text("Title disabled.")
            }
            _ => unreachable!("title toggle operation is validated by the caller"),
        };
        Ok(Some(response))
    }

    async fn handle_title(
        &self,
        group: GroupKey,
        user_id: &Id,
        role: Role,
        args: &[CommandArg],
    ) -> Result<Option<QuickOperation>, BotError> {
        let Some((target, title)) = title_args(args) else {
            return Ok(Some(text("Usage: .title @member title")));
        };
        if !self.can_administer(user_id, role) {
            return Ok(Some(text("Permission denied.")));
        }
        if !self.is_feature_enabled(&group, TITLE).await {
            return Ok(Some(text("Title is disabled in this group.")));
        }

        let group_id = Id::new(group.group_id).expect("group key contains a valid ID");
        self.napcat
            .set_group_special_title(&group_id, target, &title)
            .await?;
        Ok(Some(text("Title updated.")))
    }

    async fn group_lock(&self, group: &GroupKey) -> Arc<Mutex<()>> {
        self.group_locks
            .lock()
            .await
            .entry(group.clone())
            .or_insert_with(|| Arc::new(Mutex::new(())))
            .clone()
    }

    async fn is_feature_enabled(&self, group: &GroupKey, feature_name: &str) -> bool {
        self.features
            .read()
            .await
            .get(&group.feature(feature_name))
            .is_some_and(|record| record.enabled)
    }

    async fn dice_session_mode(&self, group: &GroupKey) -> DiceSessionMode {
        self.features
            .read()
            .await
            .get(&group.feature(DICE))
            .map(|record| DiceSessionMode::from_manifest(&record.manifest))
            .unwrap_or_default()
    }

    async fn set_feature_manifest(
        &self,
        group: &GroupKey,
        feature_name: &str,
        key: &str,
        value: &str,
    ) -> Result<QuickOperation, StoreError> {
        if !known_feature(feature_name) {
            return Ok(text("Unknown feature."));
        }
        if key.contains('.') {
            return Ok(text("Nested manifest keys are not supported."));
        }

        let feature = group.feature(feature_name);
        let Some(record) = self
            .features
            .read()
            .await
            .get(&feature)
            .filter(|record| record.enabled)
            .cloned()
        else {
            return Ok(text("Feature is disabled."));
        };
        if feature_name != DICE || key != SESSION_MODE {
            return Ok(text("Unknown manifest key."));
        }
        let Some(value) = DiceSessionMode::parse(value) else {
            return Ok(text("Invalid manifest value."));
        };

        let mut manifest = record.manifest;
        manifest.set_string(SESSION_MODE, value.as_str());
        let Some(record) = self.store.update_manifest(&feature, &manifest).await? else {
            return Ok(text("Feature is disabled."));
        };
        self.features.write().await.insert(feature, record);
        Ok(text("Feature manifest updated."))
    }

    async fn get_feature_manifest(
        &self,
        group: &GroupKey,
        feature_name: &str,
        key: Option<&str>,
    ) -> Result<QuickOperation, StoreError> {
        if !known_feature(feature_name) {
            return Ok(text("Unknown feature."));
        }
        if key.is_some_and(|key| key.contains('.')) {
            return Ok(text("Nested manifest keys are not supported."));
        }

        let feature = group.feature(feature_name);
        let Some(manifest) = self
            .features
            .read()
            .await
            .get(&feature)
            .filter(|record| record.enabled)
            .map(|record| record.manifest.clone())
        else {
            return Ok(text("Feature is disabled."));
        };

        let output = match key {
            Some(SESSION_MODE) if feature_name == DICE => {
                match manifest.selected_toml(SESSION_MODE)? {
                    Some(value) => value,
                    None => return Ok(text("Manifest key is not set.")),
                }
            }
            Some(_) => return Ok(text("Unknown manifest key.")),
            None if manifest.values().is_empty() => {
                return Ok(text("Feature manifest is empty."));
            }
            None => manifest.to_toml()?,
        };
        Ok(QuickOperation::text(output))
    }

    fn can_administer(&self, user_id: &Id, role: Role) -> bool {
        self.owners.contains(user_id.as_str()) || role.privileged()
    }
}

fn only_feature(args: &[CommandArg], feature_name: &str) -> bool {
    matches!(args, [CommandArg::Text(value)] if value == feature_name)
}

fn known_feature(feature_name: &str) -> bool {
    matches!(feature_name, DICE | TITLE)
}

fn title_args(args: &[CommandArg]) -> Option<(&Id, String)> {
    let [CommandArg::At(target), title @ ..] = args else {
        return None;
    };
    let title = title
        .iter()
        .map(|arg| match arg {
            CommandArg::Text(value) => Some(value.as_str()),
            _ => None,
        })
        .collect::<Option<Vec<_>>>()?;
    (!title.is_empty()).then(|| (target, title.join(" ")))
}

fn fset_args(args: &[CommandArg]) -> Option<(&str, &str, &str)> {
    match args {
        [
            CommandArg::Text(feature),
            CommandArg::Text(key),
            CommandArg::Text(value),
        ] => Some((feature, key, value)),
        _ => None,
    }
}

fn fget_args(args: &[CommandArg]) -> Option<(&str, Option<&str>)> {
    match args {
        [CommandArg::Text(feature)] => Some((feature, None)),
        [CommandArg::Text(feature), CommandArg::Text(key)] => Some((feature, Some(key))),
        _ => None,
    }
}

fn text(value: &'static str) -> QuickOperation {
    QuickOperation::text(value)
}

fn statistics_message(rolls: &[(Id, u8)]) -> Vec<MessageSegment> {
    let mut message = Vec::new();
    for score in (1..=6).rev() {
        let prefix = if score == 6 {
            format!("{score}:")
        } else {
            format!("\n{score}:")
        };
        message.push(MessageSegment::text(prefix));
        for (user_id, _) in rolls.iter().filter(|(_, result)| *result == score) {
            message.push(MessageSegment::text(" "));
            message.push(MessageSegment::at(user_id));
        }
    }
    message
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex as StdMutex};

    use axum::{Json, Router, routing::post};
    use serde_json::json;
    use tempfile::tempdir;

    use super::*;
    use crate::protocol::Sender;

    async fn test_bot() -> Bot {
        let directory = tempdir().unwrap().keep();
        let url = format!(
            "sqlite://{}?mode=rwc",
            directory.join("bot.sqlite").display()
        );
        let config = Config::for_test(url.clone());
        let store = FeatureStore::connect(&url).await.unwrap();
        Bot::new(&config, store, HashMap::new())
    }

    async fn test_bot_with_enabled_dice(manifest: Option<&str>) -> Bot {
        let directory = tempdir().unwrap().keep();
        let url = format!(
            "sqlite://{}?mode=rwc",
            directory.join("bot.sqlite").display()
        );
        let config = Config::for_test(url.clone());
        let store = FeatureStore::connect(&url).await.unwrap();
        let feature = GroupKey {
            self_id: "999".into(),
            group_id: "200".into(),
        }
        .feature(DICE);
        store.enable(&feature, "10000").await.unwrap();
        if let Some(source) = manifest {
            let manifest = FeatureManifest::from_toml(source).unwrap();
            store.update_manifest(&feature, &manifest).await.unwrap();
        }
        let features = store.load_all().await.unwrap();
        Bot::new(&config, store, features)
    }

    fn event(user: &str, role: &str, message: Vec<MessageSegment>) -> Event {
        Event {
            post_type: "message".into(),
            message_type: Some("group".into()),
            sub_type: Some("normal".into()),
            self_id: Id::new("999").unwrap(),
            message_id: Some(json!(1)),
            user_id: Some(Id::new(user).unwrap()),
            group_id: Some(Id::new("200").unwrap()),
            message,
            sender: Some(Sender {
                role: Some(role.into()),
            }),
        }
    }

    fn dice(result: &str) -> MessageSegment {
        MessageSegment {
            kind: "dice".into(),
            data: json!({"result": result}),
        }
    }

    #[test]
    fn title_arguments_require_a_mention_and_join_trailing_text() {
        let target = Id::new("42").unwrap();
        assert_eq!(
            title_args(&[
                CommandArg::At(target.clone()),
                CommandArg::Text("best".into()),
                CommandArg::Text("member".into()),
            ]),
            Some((&target, "best member".into()))
        );
        assert_eq!(title_args(&[CommandArg::At(target.clone())]), None);
        assert_eq!(
            title_args(&[CommandArg::At(target), CommandArg::Segment("image".into()),]),
            None
        );
    }

    #[tokio::test]
    async fn title_assignment_requires_valid_usage_permission_and_enabled_feature() {
        let bot = test_bot().await;
        for (user, role, message, expected) in [
            (
                "10000",
                "member",
                vec![MessageSegment::text(".title nobody")],
                "Usage: .title @member title",
            ),
            (
                "20",
                "member",
                vec![
                    MessageSegment::text(".title "),
                    MessageSegment::at(&Id::new("42").unwrap()),
                    MessageSegment::text(" best"),
                ],
                "Permission denied.",
            ),
            (
                "10000",
                "member",
                vec![
                    MessageSegment::text(".title "),
                    MessageSegment::at(&Id::new("42").unwrap()),
                    MessageSegment::text(" best"),
                ],
                "Title is disabled in this group.",
            ),
        ] {
            let response = bot
                .process(event(user, role, message))
                .await
                .unwrap()
                .unwrap();
            assert_eq!(response, QuickOperation::text(expected));
        }
    }

    #[tokio::test]
    #[ignore = "requires loopback networking"]
    async fn title_toggles_require_the_bot_to_own_the_group() {
        let bot_role = Arc::new(StdMutex::new("member"));
        let response_role = bot_role.clone();
        let title_request = Arc::new(StdMutex::new(None));
        let captured_title_request = title_request.clone();
        let app = Router::new()
            .route(
                "/get_group_member_info",
                post(move || {
                    let role = *response_role.lock().unwrap();
                    async move {
                        Json(json!({
                            "status": "ok", "retcode": 0, "data": {"role": role},
                            "message": "", "wording": ""
                        }))
                    }
                }),
            )
            .route(
                "/set_group_special_title",
                post(move |Json(body): Json<serde_json::Value>| {
                    *captured_title_request.lock().unwrap() = Some(body);
                    async {
                        Json(json!({
                            "status": "ok", "retcode": 0, "data": {},
                            "message": "", "wording": ""
                        }))
                    }
                }),
            );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

        let directory = tempdir().unwrap();
        let url = format!(
            "sqlite://{}?mode=rwc",
            directory.path().join("title.sqlite").display()
        );
        let mut config = Config::for_test(url.clone());
        config.napcat_base_url = format!("http://{address}");
        let store = FeatureStore::connect(&url).await.unwrap();
        let bot = Bot::new(&config, store, HashMap::new());
        let group = GroupKey {
            self_id: "999".into(),
            group_id: "200".into(),
        };

        let denied = bot
            .process(event(
                "10000",
                "member",
                vec![MessageSegment::text(".enable title")],
            ))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            denied,
            QuickOperation::text("Title feature requires the bot to be the group owner.")
        );
        assert!(!bot.is_feature_enabled(&group, TITLE).await);

        *bot_role.lock().unwrap() = "owner";
        let enabled = bot
            .process(event(
                "10000",
                "member",
                vec![MessageSegment::text(".enable title")],
            ))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(enabled, QuickOperation::text("Title enabled."));
        assert!(bot.is_feature_enabled(&group, TITLE).await);

        let updated = bot
            .process(event(
                "51",
                "admin",
                vec![
                    MessageSegment::text(".title "),
                    MessageSegment::at(&Id::new("42").unwrap()),
                    MessageSegment::text(" best member"),
                ],
            ))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(updated, QuickOperation::text("Title updated."));
        assert_eq!(
            *title_request.lock().unwrap(),
            Some(json!({
                "group_id": "200", "user_id": "42", "special_title": "best member"
            }))
        );

        *bot_role.lock().unwrap() = "member";
        let denied = bot
            .process(event(
                "10000",
                "member",
                vec![MessageSegment::text(".disable title")],
            ))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            denied,
            QuickOperation::text("Title feature requires the bot to be the group owner.")
        );
        assert!(bot.is_feature_enabled(&group, TITLE).await);
        server.abort();
    }

    #[tokio::test]
    async fn records_only_the_first_roll_and_formats_all_scores() {
        let bot = test_bot().await;
        let enable = bot
            .process(event(
                "10000",
                "member",
                vec![MessageSegment::text(".enable dice")],
            ))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(enable, QuickOperation::text("Dice statistics enabled."));
        bot.process(event("10", "member", vec![MessageSegment::text(".dice")]))
            .await
            .unwrap();
        bot.process(event("10", "member", vec![dice("2")]))
            .await
            .unwrap();
        bot.process(event("10", "member", vec![dice("6")]))
            .await
            .unwrap();
        bot.process(event("11", "member", vec![dice("6")]))
            .await
            .unwrap();
        let stats = bot
            .process(event("12", "member", vec![MessageSegment::text(".ecid")]))
            .await
            .unwrap()
            .unwrap();

        assert!(!stats.at_sender);
        assert_eq!(
            stats.reply,
            vec![
                MessageSegment::text("6:"),
                MessageSegment::text(" "),
                MessageSegment::at(&Id::new("11").unwrap()),
                MessageSegment::text("\n5:"),
                MessageSegment::text("\n4:"),
                MessageSegment::text("\n3:"),
                MessageSegment::text("\n2:"),
                MessageSegment::text(" "),
                MessageSegment::at(&Id::new("10").unwrap()),
                MessageSegment::text("\n1:"),
            ]
        );
    }

    #[tokio::test]
    async fn members_cannot_toggle_the_feature_but_configured_owners_can() {
        let bot = test_bot().await;
        let denied = bot
            .process(event(
                "20",
                "member",
                vec![MessageSegment::text(".enable dice")],
            ))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(denied, QuickOperation::text("Permission denied."));

        let enabled = bot
            .process(event(
                "10000",
                "member",
                vec![MessageSegment::text(".enable dice")],
            ))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(enabled, QuickOperation::text("Dice statistics enabled."));
    }

    #[tokio::test]
    async fn disabling_discards_an_active_session() {
        let bot = test_bot().await;
        bot.process(event(
            "21",
            "admin",
            vec![MessageSegment::text(".enable dice")],
        ))
        .await
        .unwrap();
        bot.process(event("22", "member", vec![MessageSegment::text(".dice")]))
            .await
            .unwrap();
        bot.process(event("22", "member", vec![dice("4")]))
            .await
            .unwrap();
        bot.process(event(
            "21",
            "admin",
            vec![MessageSegment::text(".disable dice")],
        ))
        .await
        .unwrap();

        let disabled = bot
            .process(event("22", "member", vec![MessageSegment::text(".ecid")]))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            disabled,
            QuickOperation::text("Dice statistics is disabled in this group.")
        );
    }

    #[tokio::test]
    async fn strict_session_mode_rejects_dice_during_an_active_session() {
        let bot = test_bot_with_enabled_dice(Some("session_mode = \"strict\"\n")).await;
        bot.process(event("30", "member", vec![MessageSegment::text(".dice")]))
            .await
            .unwrap();
        bot.process(event("30", "member", vec![dice("4")]))
            .await
            .unwrap();

        let response = bot
            .process(event("31", "member", vec![MessageSegment::text(".dice")]))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            response,
            QuickOperation::text("A dice session is already active.")
        );

        let stats = bot
            .process(event("31", "member", vec![MessageSegment::text(".ecid")]))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            stats,
            QuickOperation::reply(statistics_message(&[(Id::new("30").unwrap(), 4)]))
        );
    }

    #[tokio::test]
    async fn common_session_mode_publishes_statistics_and_starts_a_new_session() {
        for manifest in [None, Some("session_mode = \"common\"\n")] {
            let bot = test_bot_with_enabled_dice(manifest).await;
            bot.process(event("40", "member", vec![MessageSegment::text(".dice")]))
                .await
                .unwrap();
            bot.process(event("40", "member", vec![dice("5")]))
                .await
                .unwrap();

            let stats = bot
                .process(event("41", "member", vec![MessageSegment::text(".dice")]))
                .await
                .unwrap()
                .unwrap();
            assert_eq!(
                stats,
                QuickOperation::reply(statistics_message(&[(Id::new("40").unwrap(), 5)]))
            );

            bot.process(event("41", "member", vec![dice("2")]))
                .await
                .unwrap();
            let new_stats = bot
                .process(event("42", "member", vec![MessageSegment::text(".ecid")]))
                .await
                .unwrap()
                .unwrap();
            assert_eq!(
                new_stats,
                QuickOperation::reply(statistics_message(&[(Id::new("41").unwrap(), 2)]))
            );
        }
    }

    #[tokio::test]
    async fn manifest_commands_persist_values_and_update_dice_behavior() {
        let bot = test_bot_with_enabled_dice(None).await;

        let empty = bot
            .process(event(
                "50",
                "member",
                vec![MessageSegment::text(".fget dice")],
            ))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(empty, QuickOperation::text("Feature manifest is empty."));
        let unset = bot
            .process(event(
                "50",
                "member",
                vec![MessageSegment::text(".fget dice session_mode")],
            ))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(unset, QuickOperation::text("Manifest key is not set."));

        let denied = bot
            .process(event(
                "50",
                "member",
                vec![MessageSegment::text(".fset dice session_mode strict")],
            ))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(denied, QuickOperation::text("Permission denied."));

        let updated = bot
            .process(event(
                "51",
                "admin",
                vec![MessageSegment::text(".fset dice session_mode strict")],
            ))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(updated, QuickOperation::text("Feature manifest updated."));
        for command in [".fget dice", ".fget dice session_mode"] {
            let value = bot
                .process(event("50", "member", vec![MessageSegment::text(command)]))
                .await
                .unwrap()
                .unwrap();
            assert_eq!(value, QuickOperation::text("session_mode = \"strict\"\n"));
        }
        let stored = bot.store.load_all().await.unwrap();
        assert_eq!(
            stored[&GroupKey {
                self_id: "999".into(),
                group_id: "200".into(),
            }
            .feature(DICE)]
                .manifest
                .get(SESSION_MODE),
            Some(&toml::Value::String("strict".into()))
        );

        bot.process(event("50", "member", vec![MessageSegment::text(".dice")]))
            .await
            .unwrap();
        let strict = bot
            .process(event("50", "member", vec![MessageSegment::text(".dice")]))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            strict,
            QuickOperation::text("A dice session is already active.")
        );

        bot.process(event(
            "51",
            "admin",
            vec![MessageSegment::text(".fset dice session_mode common")],
        ))
        .await
        .unwrap();
        let common = bot
            .process(event("50", "member", vec![MessageSegment::text(".dice")]))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(common, QuickOperation::reply(statistics_message(&[])));
    }

    #[tokio::test]
    async fn manifest_commands_validate_feature_schema_and_usage() {
        let bot = test_bot().await;
        for (command, expected) in [
            (".fget unknown", "Unknown feature."),
            (".fset unknown key value", "Unknown feature."),
            (".fget dice", "Feature is disabled."),
            (".fset dice session_mode strict", "Feature is disabled."),
            (".fget", "Usage: .fget feature [key]"),
            (".fget dice one two", "Usage: .fget feature [key]"),
            (".fset dice session_mode", "Usage: .fset feature key value"),
            (
                ".fset dice session_mode strict extra",
                "Usage: .fset feature key value",
            ),
        ] {
            let role = if command.starts_with(".fset") {
                "admin"
            } else {
                "member"
            };
            let response = bot
                .process(event(
                    role_id(role),
                    role,
                    vec![MessageSegment::text(command)],
                ))
                .await
                .unwrap()
                .unwrap();
            assert_eq!(response, QuickOperation::text(expected), "{command}");
        }

        bot.process(event(
            "51",
            "admin",
            vec![MessageSegment::text(".enable dice")],
        ))
        .await
        .unwrap();
        for (command, expected) in [
            (
                ".fset dice session.strict true",
                "Nested manifest keys are not supported.",
            ),
            (
                ".fget dice session.strict",
                "Nested manifest keys are not supported.",
            ),
            (".fset dice unknown value", "Unknown manifest key."),
            (".fget dice unknown", "Unknown manifest key."),
            (
                ".fset dice session_mode permissive",
                "Invalid manifest value.",
            ),
        ] {
            let role = if command.starts_with(".fset") {
                "admin"
            } else {
                "member"
            };
            let response = bot
                .process(event(
                    role_id(role),
                    role,
                    vec![MessageSegment::text(command)],
                ))
                .await
                .unwrap()
                .unwrap();
            assert_eq!(response, QuickOperation::text(expected), "{command}");
        }

        let ignored = bot
            .process(event(
                "51",
                "admin",
                vec![MessageSegment::text(
                    ".fseta dice session.options[0] strict",
                )],
            ))
            .await
            .unwrap();
        assert_eq!(ignored, None);
    }

    fn role_id(role: &str) -> &str {
        if role == "admin" { "51" } else { "50" }
    }
}
