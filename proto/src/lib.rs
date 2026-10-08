//! Wire protocol between the TUI and the bridge: one JSON object per line.

use serde::{Deserialize, Serialize};

pub const PROTOCOL_VERSION: u32 = 3;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Request {
    pub id: u64,
    #[serde(flatten)]
    pub cmd: Command,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "cmd", rename_all = "snake_case")]
pub enum Command {
    ListChats {
        limit: u32,
    },
    History {
        chat_id: i64,
        /// Further chat rows shown together with `chat_id` (the same person's other handles).
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        chat_ids: Vec<i64>,
        #[serde(default)]
        before_rowid: Option<i64>,
        limit: u32,
    },
    Since {
        rowid: i64,
    },
    /// Which handles belong to one person, from the Mac's Contacts.
    People {
        #[serde(default)]
        names: bool,
    },
    Send {
        #[serde(default)]
        to: Option<String>,
        #[serde(default)]
        chat_guid: Option<String>,
        text: String,
    },
    /// Everything up to `rowid` in these chat rows has been read in a TUI.
    MarkRead {
        chat_ids: Vec<i64>,
        rowid: i64,
    },
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Response {
    pub id: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chats: Option<Vec<Chat>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub messages: Option<Vec<Message>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub people: Option<Vec<PersonGroup>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ok: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub service: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl Response {
    pub fn error(id: u64, msg: impl Into<String>) -> Self {
        Response {
            id,
            error: Some(msg.into()),
            ..Default::default()
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum Event {
    Hello {
        version: u32,
        bridge: String,
    },
    NewMessage {
        message: Message,
    },
    MessageUpdated {
        message: Message,
    },
    /// A client read these chat rows up to `rowid`.
    Read {
        chat_ids: Vec<i64>,
        rowid: i64,
    },
    Error {
        msg: String,
    },
}

/// Anything the bridge writes to a client.
#[derive(Debug, Clone)]
pub enum ServerMsg {
    Event(Event),
    Response(Response),
}

/// Lines with an `event` key are events; everything else is a response.
pub fn parse_server_line(line: &str) -> Result<ServerMsg, serde_json::Error> {
    let v: serde_json::Value = serde_json::from_str(line)?;
    if v.get("event").is_some() {
        Ok(ServerMsg::Event(serde_json::from_value(v)?))
    } else {
        Ok(ServerMsg::Response(serde_json::from_value(v)?))
    }
}

/// chat.db handles that one Contacts card (or set of linked cards) covers.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PersonGroup {
    pub handles: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Chat {
    pub chat_id: i64,
    pub guid: String,
    pub display: String,
    pub is_group: bool,
    pub service: String,
    /// Participant handles (phone numbers / emails), excluding you.
    pub handles: Vec<String>,
    pub unread: u32,
    pub last: Option<Message>,
    /// Other chat rows the TUI merged into this one; the bridge leaves it empty.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub chat_ids: Vec<i64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    Text,
    Tapback,
    Attachment,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Message {
    pub rowid: i64,
    pub chat_id: i64,
    /// Sender handle for incoming messages; `None` when from you.
    pub sender: Option<String>,
    pub from_me: bool,
    pub text: String,
    /// Unix seconds.
    pub ts: i64,
    pub service: String,
    pub is_error: bool,
    pub edited: bool,
    pub unsent: bool,
    /// For messages you sent: when it reached the recipient (unix seconds).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub delivered: Option<i64>,
    /// For messages you sent: when the recipient read it (unix seconds).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub read: Option<i64>,
    pub kind: Kind,
    #[serde(default)]
    pub attachments: Vec<Attachment>,
    #[serde(default)]
    pub tapback: Option<Tapback>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Attachment {
    pub name: String,
    pub mime: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Tapback {
    pub emoji: String,
    pub target_text: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_roundtrip() {
        let r: Request =
            serde_json::from_str(r#"{"id":3,"cmd":"send","to":"+15550000001","text":"hi"}"#)
                .unwrap();
        assert_eq!(r.id, 3);
        assert!(
            matches!(r.cmd, Command::Send { ref to, .. } if to.as_deref() == Some("+15550000001"))
        );
        let h: Request =
            serde_json::from_str(r#"{"id":1,"cmd":"history","chat_id":2,"limit":50}"#).unwrap();
        assert!(matches!(
            h.cmd,
            Command::History {
                before_rowid: None,
                ..
            }
        ));
    }

    #[test]
    fn server_line_dispatch() {
        let e = parse_server_line(r#"{"event":"hello","version":1,"bridge":"0.1.0"}"#).unwrap();
        assert!(matches!(
            e,
            ServerMsg::Event(Event::Hello { version: 1, .. })
        ));
        let r = parse_server_line(r#"{"id":7,"ok":true,"service":"iMessage"}"#).unwrap();
        assert!(matches!(
            r,
            ServerMsg::Response(Response {
                id: 7,
                ok: Some(true),
                ..
            })
        ));
    }
}
