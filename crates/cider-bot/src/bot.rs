use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
};

use tokio::sync::{Mutex, RwLock};

use crate::{
    command::{self, Command, CommandArg},
    config::Config,
    napcat::NapcatClient,
    protocol::{Event, Id, MessageSegment, QuickOperation},
    store::{FeatureKey, FeatureRecord, FeatureStore, StoreError},
};

const DICE: &str = "dice";

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

    fn feature(&self) -> FeatureKey {
        FeatureKey {
            self_id: self.self_id.clone(),
            group_id: self.group_id.clone(),
            feature_name: DICE.into(),
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

    pub async fn process(&self, event: Event) -> Result<Option<QuickOperation>, StoreError> {
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
        let lock = self.group_lock(&group).await;
        let _guard = lock.lock().await;

        if let Some(command) = command::parse(&event.message) {
            let role = Role::parse(
                event
                    .sender
                    .as_ref()
                    .and_then(|sender| sender.role.as_deref()),
            );
            return self.handle_command(group, user_id, role, command).await;
        }

        let Some(result) = event.message.iter().find_map(MessageSegment::dice_result) else {
            return Ok(None);
        };
        if !self.is_enabled(&group).await {
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
    ) -> Result<Option<QuickOperation>, StoreError> {
        let response = match command.name.as_str() {
            "enable" => {
                if !only_dice(&command.args) {
                    text("Usage: .enable dice")
                } else if !self.can_administer(user_id, role) {
                    text("Permission denied.")
                } else if self.is_enabled(&group).await {
                    text("Dice statistics is already enabled.")
                } else {
                    let feature = group.feature();
                    let record = self.store.enable(&feature, user_id.as_str()).await?;
                    self.features.write().await.insert(feature, record);
                    text("Dice statistics enabled.")
                }
            }
            "disable" => {
                if !only_dice(&command.args) {
                    text("Usage: .disable dice")
                } else if !self.can_administer(user_id, role) {
                    text("Permission denied.")
                } else if !self.is_enabled(&group).await {
                    text("Dice statistics is already disabled.")
                } else {
                    let feature = group.feature();
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
                } else if !self.is_enabled(&group).await {
                    text("Dice statistics is disabled in this group.")
                } else {
                    let mut sessions = self.sessions.lock().await;
                    if let std::collections::hash_map::Entry::Vacant(entry) = sessions.entry(group)
                    {
                        entry.insert(DiceSession::default());
                        text("Dice session started.")
                    } else {
                        text("A dice session is already active.")
                    }
                }
            }
            "ecid" => {
                if !command.args.is_empty() {
                    text("Usage: .ecid")
                } else if !self.is_enabled(&group).await {
                    text("Dice statistics is disabled in this group.")
                } else if let Some(session) = self.sessions.lock().await.remove(&group) {
                    QuickOperation::reply(statistics_message(&session.rolls))
                } else {
                    text("No dice session is active.")
                }
            }
            _ => return Ok(None),
        };
        Ok(Some(response))
    }

    async fn group_lock(&self, group: &GroupKey) -> Arc<Mutex<()>> {
        self.group_locks
            .lock()
            .await
            .entry(group.clone())
            .or_insert_with(|| Arc::new(Mutex::new(())))
            .clone()
    }

    async fn is_enabled(&self, group: &GroupKey) -> bool {
        self.features
            .read()
            .await
            .get(&group.feature())
            .is_some_and(|record| record.enabled)
    }

    fn can_administer(&self, user_id: &Id, role: Role) -> bool {
        self.owners.contains(user_id.as_str()) || role.privileged()
    }
}

fn only_dice(args: &[CommandArg]) -> bool {
    matches!(args, [CommandArg::Text(value)] if value == DICE)
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
}
