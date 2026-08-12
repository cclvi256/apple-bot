use crate::protocol::{Id, MessageSegment};

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CommandArg {
    Text(String),
    At(Id),
    Segment(String),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Command {
    pub name: String,
    pub args: Vec<CommandArg>,
}

pub fn parse(segments: &[MessageSegment]) -> Option<Command> {
    let first = segments.iter().position(is_text)?;
    let last = segments.iter().rposition(is_text)?;
    let mut args = Vec::new();
    let mut text = String::new();

    let flush_text = |text: &mut String, args: &mut Vec<CommandArg>| {
        args.extend(
            text.split_whitespace()
                .map(|value| CommandArg::Text(value.to_owned())),
        );
        text.clear();
    };

    for segment in &segments[first..=last] {
        if let Some(value) = segment.text_value() {
            text.push_str(value);
            continue;
        }

        flush_text(&mut text, &mut args);
        if let Some(qq) = segment.at_value() {
            args.push(CommandArg::At(qq));
        } else {
            args.push(CommandArg::Segment(segment.kind.clone()));
        }
    }
    flush_text(&mut text, &mut args);

    let CommandArg::Text(command) = args.first()? else {
        return None;
    };
    let name = command.strip_prefix('.')?;
    if name.is_empty() {
        return None;
    }

    Some(Command {
        name: name.to_owned(),
        args: args.into_iter().skip(1).collect(),
    })
}

fn is_text(segment: &MessageSegment) -> bool {
    segment.text_value().is_some()
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn segment(kind: &str, data: serde_json::Value) -> MessageSegment {
        MessageSegment {
            kind: kind.into(),
            data,
        }
    }

    #[test]
    fn trims_leading_mention_and_first_text_whitespace() {
        let input = vec![
            segment("at", json!({"qq":"10000"})),
            MessageSegment::text("   .enable dice  "),
        ];
        assert_eq!(
            parse(&input),
            Some(Command {
                name: "enable".into(),
                args: vec![CommandArg::Text("dice".into())],
            })
        );
    }

    #[test]
    fn joins_adjacent_text_and_preserves_embedded_segments() {
        let input = vec![
            MessageSegment::text(".fo"),
            MessageSegment::text("o "),
            segment("at", json!({"qq":"123"})),
            MessageSegment::text(" tail"),
        ];
        assert_eq!(
            parse(&input),
            Some(Command {
                name: "foo".into(),
                args: vec![
                    CommandArg::At(Id::new("123").unwrap()),
                    CommandArg::Text("tail".into())
                ],
            })
        );
    }
}
