//! The command line: which config to read, and messages written by hand on the server.

use crate::app::now_ms;
use crate::config::Config;
use crate::store::{Error, Store};
use crate::{decks, notifications};

const USAGE: &str = "usage: ankiquest [config.json] [message <player> <text> [--from <player>] [--title <text>]]\n       ankiquest [config.json] admin <command>   (see ankiquest admin help)";

#[derive(Debug, PartialEq)]
pub(crate) struct Message {
    to: String,
    text: String,
    from: Option<String>,
    title: Option<String>,
}

/// Splits `[config] [message ...]` so the same binary serves and speaks.
pub(crate) fn parse_args(args: &[String]) -> Result<(Option<String>, Option<Message>), Error> {
    let (path, rest) = match args.first().map(String::as_str) {
        Some("message") => (None, args),
        Some(first) if !first.starts_with('-') => (Some(first.to_string()), &args[1..]),
        Some(_) => return Err(USAGE.into()),
        None => return Ok((None, None)),
    };
    let rest = match rest.first().map(String::as_str) {
        Some("message") => &rest[1..],
        Some(_) => return Err(USAGE.into()),
        None => return Ok((path, None)),
    };

    let mut positional = Vec::new();
    let mut from = None;
    let mut title = None;
    let mut rest = rest.iter();
    while let Some(argument) = rest.next() {
        let mut value = |name: &str| {
            rest.next()
                .cloned()
                .ok_or_else(|| Error::from(format!("{name} needs a value")))
        };
        match argument.as_str() {
            "--from" => from = Some(value("--from")?),
            "--title" => title = Some(value("--title")?),
            other if other.starts_with("--") => {
                return Err(format!("unknown option {other}").into());
            }
            other => positional.push(other.to_string()),
        }
    }
    let [to, text] = positional.as_slice() else {
        return Err(USAGE.into());
    };
    Ok((
        path,
        Some(Message {
            to: to.clone(),
            text: text.trim().to_string(),
            from,
            title,
        }),
    ))
}

/// Delivers a message written by hand on the server, replyable when it says who sent it.
pub(crate) fn send_message(config: &Config, message: &Message) -> Result<(), Error> {
    let known = |user: &str| config.users.contains_key(user);
    if !known(&message.to) {
        return Err(format!("{} is not a configured player", message.to).into());
    }
    if let Some(from) = message.from.as_deref().filter(|from| !known(from)) {
        return Err(format!("{from} is not a configured player").into());
    }
    if !decks::valid_message(&message.text) {
        return Err(format!(
            "the message must be 1 to {} characters on a single line",
            decks::MAX_MESSAGE
        )
        .into());
    }
    let title = match (&message.title, &message.from) {
        (Some(title), _) => title.clone(),
        (None, Some(from)) => format!("\u{1f4ac} {}", display_of(config, from)),
        (None, None) => "ankiquest".to_string(),
    };
    let mut store = Store::open(&config.state_dir)?;
    let now = now_ms();
    let day = store.clock(&message.to)?.day(now);
    let id = store.send(
        &notifications::Outgoing {
            to: &message.to,
            from: message.from.as_deref().unwrap_or(""),
            title: &title,
            body: &message.text,
            kind: "message",
        },
        day,
        now,
    )?;
    println!("delivered to {} as notification {id}", message.to);
    Ok(())
}

fn display_of(config: &Config, user: &str) -> String {
    config
        .users
        .get(user)
        .and_then(|u| u.display.clone())
        .unwrap_or_else(|| user.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::inbox::{ReplyRequest, get_notifications, reply};
    use crate::test_support::*;

    fn words(parts: &[&str]) -> Vec<String> {
        parts.iter().map(|part| (*part).to_string()).collect()
    }

    #[test]
    fn the_message_command_reads_a_config_and_its_options_in_any_order() {
        assert_eq!(parse_args(&words(&[])).unwrap(), (None, None));
        let (path, nothing) = parse_args(&words(&["/etc/ankiquest.json"])).unwrap();
        assert_eq!(path.as_deref(), Some("/etc/ankiquest.json"));
        assert_eq!(nothing, None);

        let (path, message) = parse_args(&words(&[
            "/etc/ankiquest.json",
            "message",
            "--from",
            "cerro",
            "aldanita",
            "  keep going  ",
            "--title",
            "Hi",
        ]))
        .unwrap();
        assert_eq!(path.as_deref(), Some("/etc/ankiquest.json"));
        assert_eq!(
            message,
            Some(Message {
                to: "aldanita".into(),
                text: "keep going".into(),
                from: Some("cerro".into()),
                title: Some("Hi".into()),
            })
        );

        let (path, message) = parse_args(&words(&["message", "aldanita", "hello"])).unwrap();
        assert_eq!(path, None, "the config then comes from the environment");
        assert_eq!(message.unwrap().from, None);

        for bad in [
            vec!["message"],
            vec!["message", "aldanita"],
            vec!["message", "aldanita", "hello", "spare"],
            vec!["message", "aldanita", "hello", "--from"],
            vec!["message", "aldanita", "hello", "--shout"],
            vec!["--help"],
            vec!["/etc/ankiquest.json", "serve"],
        ] {
            assert!(
                parse_args(&words(&bad)).is_err(),
                "{bad:?} should not parse"
            );
        }
    }

    #[tokio::test]
    async fn a_message_written_on_the_server_lands_in_the_inbox_and_can_be_answered() {
        let (app, path) = fixture();
        let config: Config = serde_json::from_value(serde_json::json!({
            "state_dir": path,
            "users": {
                "cerro": {"display": "Cerro", "token": "cerro-secret"},
                "hill": {"display": "Hill", "token": "hill-secret"},
            }
        }))
        .unwrap();
        let note = |to: &str, from: Option<&str>| Message {
            to: to.into(),
            text: "you're doing great, keep going".into(),
            from: from.map(str::to_string),
            title: None,
        };
        send_message(&config, &note("hill", Some("cerro"))).unwrap();

        let inbox = get_notifications(State(app.clone()), UrlPath("hill".into()), headers("hill"))
            .await
            .unwrap()
            .0;
        assert_eq!(inbox.len(), 1);
        assert_eq!(inbox[0].title, "\u{1f4ac} Cerro");
        assert_eq!(inbox[0].kind, "message", "written by hand, not by the game");
        assert_eq!(inbox[0].body, "you're doing great, keep going");
        assert_eq!(inbox[0].sender, "cerro");
        assert_eq!(
            reply(
                State(app.clone()),
                UrlPath("hill".into()),
                headers("hill"),
                Json(ReplyRequest {
                    notification: inbox[0].id,
                    message: "thank you!".into()
                })
            )
            .await
            .unwrap()
            .0
            .sent_to,
            "Cerro"
        );

        send_message(&config, &note("hill", None)).unwrap();
        let anonymous =
            get_notifications(State(app.clone()), UrlPath("hill".into()), headers("hill"))
                .await
                .unwrap()
                .0
                .pop()
                .unwrap();
        assert_eq!(anonymous.title, "ankiquest");
        assert!(anonymous.sender.is_empty(), "nobody to answer");

        assert!(send_message(&config, &note("nobody", None)).is_err());
        assert!(send_message(&config, &note("hill", Some("nobody"))).is_err());
        for text in ["", "   ", "two\nlines", &"x".repeat(decks::MAX_MESSAGE + 1)] {
            let mut empty = note("hill", None);
            empty.text = text.into();
            assert!(
                send_message(&config, &empty).is_err(),
                "{text:?} is not a message"
            );
        }
        drop(app);
        std::fs::remove_dir_all(path).unwrap();
    }
}
