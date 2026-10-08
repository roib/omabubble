//! Application state and logic. No terminal or process I/O here: side effects
//! are queued in `effects` and carried out by the main loop.

use crate::config::{Config, Person};
use crate::phone;
use chrono::{Local, TimeZone};
use proto::{
    Chat, Command, Event, Kind, Message, PROTOCOL_VERSION, PersonGroup, Request, Response,
    ServerMsg, parse_server_line,
};
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use std::collections::HashMap;
use std::time::{Duration, Instant};

pub const HISTORY_PAGE: u32 = 50;
const CHAT_LIMIT: u32 = 200;
const UNCONFIRMED_AFTER: Duration = Duration::from_secs(30);
const FIRST_BACKOFF: Duration = Duration::from_secs(1);
const MAX_BACKOFF: Duration = Duration::from_secs(30);
const FLASH_FOR: Duration = Duration::from_secs(3);
const HINT_FOR: Duration = Duration::from_secs(12);
/// Echoes that matched no placeholder, kept for a retry after the chat list refreshes.
const UNMATCHED_KEEP: usize = 20;

#[derive(Debug)]
pub enum Effect {
    Request(Request),
    Notify { title: String, body: String },
    Bell,
    Copy(String),
    Reconnect,
    Disconnect,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Focus {
    List,
    Messages,
    Input,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Status {
    Connecting,
    Connected,
    Disconnected { reason: String, retry_at: Instant },
    Fatal(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    Chat(i64),
    /// A new conversation with a handle that has no chat yet.
    Draft(String),
}

#[derive(Debug, Clone, PartialEq)]
pub enum PendState {
    Sending,
    Failed(String),
    Unconfirmed,
}

#[derive(Debug, Clone)]
pub struct Pending {
    pub local_id: u64,
    pub target: Target,
    pub text: String,
    /// Highest rowid seen when this was sent; its echo must be newer.
    pub after_rowid: i64,
    pub sent_at: Instant,
    pub ts: i64,
    pub state: PendState,
}

/// What the message pane is showing.
#[derive(Debug)]
pub struct View {
    pub target: Target,
    pub messages: Vec<Message>,
    pub loading: bool,
    pub no_more: bool,
    /// Lines scrolled up from the bottom.
    pub scroll: usize,
    /// Selected item index (messages then pending sends) in the Messages pane.
    pub sel: Option<usize>,
    pub anchor: Option<usize>,
    /// Ask the renderer to scroll the selection into view.
    pub follow_sel: bool,
    /// Lines were added at the bottom since the last render.
    pub appended: bool,
    /// Filled in by the renderer.
    pub total_lines: usize,
    pub height: usize,
}

impl View {
    fn new(target: Target) -> View {
        View {
            target,
            messages: Vec::new(),
            loading: false,
            no_more: false,
            scroll: 0,
            sel: None,
            anchor: None,
            follow_sel: false,
            appended: false,
            total_lines: 0,
            height: 0,
        }
    }

    fn max_scroll(&self) -> usize {
        self.total_lines.saturating_sub(self.height)
    }
}

pub enum Item<'a> {
    Msg(&'a Message),
    Pending(&'a Pending),
}

#[derive(Debug, Default)]
pub struct Popup {
    pub to: String,
    pub msg: String,
    /// 0 = To, 1 = Message.
    pub field: usize,
}

#[derive(Debug)]
enum ReqKind {
    ListChats,
    People,
    History { chat_id: i64, older: bool },
    Since,
    Send { local_id: u64 },
}

pub struct App {
    pub host_label: String,
    country_code: String,
    notifications: bool,
    bell: bool,
    /// Config overrides plus the groups the bridge found in Contacts.
    people: Vec<Person>,
    config_people: Vec<Person>,
    names: bool,
    /// Chats as the bridge listed them, before merging by person.
    raw_chats: Vec<Chat>,

    pub chats: Vec<Chat>,
    pub selected: usize,
    pub view: Option<View>,
    pub pending: Vec<Pending>,

    pub focus: Focus,
    pub input: String,
    /// Byte offset into `input`.
    pub cursor: usize,
    pub popup: Option<Popup>,

    pub status: Status,
    backoff: Duration,
    last_bridge_error: Option<String>,
    /// (message, shown at, how long).
    pub flash: Option<(String, Instant, Duration)>,

    max_seen: i64,
    /// chat_id → newest rowid the user has looked at in this session.
    read_upto: HashMap<i64, i64>,
    next_id: u64,
    next_local: u64,
    inflight: HashMap<u64, ReqKind>,
    chats_dirty: bool,
    loaded_once: bool,
    unmatched: Vec<Message>,

    pub effects: Vec<Effect>,
    pub quit: bool,
}

impl App {
    pub fn new(cfg: &Config) -> App {
        let host_label = match &cfg.command {
            Some(argv) if !argv.is_empty() => {
                argv[0].rsplit('/').next().unwrap_or("local").to_string()
            }
            _ => cfg.host.clone(),
        };
        App {
            host_label,
            country_code: cfg.country_code.clone(),
            notifications: cfg.notifications,
            bell: cfg.bell,
            people: cfg.people.clone(),
            config_people: cfg.people.clone(),
            names: cfg.names,
            raw_chats: Vec::new(),
            chats: Vec::new(),
            selected: 0,
            view: None,
            pending: Vec::new(),
            focus: Focus::List,
            input: String::new(),
            cursor: 0,
            popup: None,
            status: Status::Connecting,
            backoff: FIRST_BACKOFF,
            last_bridge_error: None,
            flash: None,
            max_seen: 0,
            read_upto: HashMap::new(),
            next_id: 0,
            next_local: 0,
            inflight: HashMap::new(),
            chats_dirty: false,
            loaded_once: false,
            unmatched: Vec::new(),
            effects: Vec::new(),
            quit: false,
        }
    }

    // ---------------------------------------------------------------- connection

    pub fn on_connecting(&mut self) {
        if !matches!(self.status, Status::Fatal(_)) {
            self.status = Status::Connecting;
        }
    }

    pub fn on_closed(&mut self, reason: String, now: Instant) {
        if matches!(self.status, Status::Fatal(_)) {
            return;
        }
        let reason = self.last_bridge_error.take().unwrap_or(reason);
        self.status = Status::Disconnected {
            reason,
            retry_at: now + self.backoff,
        };
        self.backoff = (self.backoff * 2).min(MAX_BACKOFF);
        for (_, kind) in self.inflight.drain() {
            match kind {
                ReqKind::Send { local_id } => {
                    if let Some(p) = self.pending.iter_mut().find(|p| p.local_id == local_id)
                        && p.state == PendState::Sending
                    {
                        p.state = PendState::Unconfirmed;
                    }
                }
                ReqKind::History { .. } => {
                    if let Some(v) = &mut self.view {
                        v.loading = false;
                    }
                }
                _ => {}
            }
        }
    }

    pub fn on_line(&mut self, line: &str, now: Instant) {
        match parse_server_line(line) {
            Ok(ServerMsg::Event(e)) => self.on_event(e, now),
            Ok(ServerMsg::Response(r)) => self.on_response(r, now),
            Err(e) => self.flash_msg(format!("bad line from bridge: {e}"), now),
        }
    }

    fn on_event(&mut self, event: Event, now: Instant) {
        match event {
            Event::Hello { version, bridge } => {
                if version != PROTOCOL_VERSION {
                    self.status = Status::Fatal(format!(
                        "bridge {bridge} speaks protocol v{version}, this imsg speaks v{PROTOCOL_VERSION}: \
                         rebuild/reinstall imsg-bridge on the Mac"
                    ));
                    self.effects.push(Effect::Disconnect);
                    return;
                }
                self.status = Status::Connected;
                self.backoff = FIRST_BACKOFF;
                self.last_bridge_error = None;
                self.inflight.clear();
                if self.max_seen > 0 {
                    self.request(
                        Command::Since {
                            rowid: self.max_seen,
                        },
                        ReqKind::Since,
                    );
                }
                self.request(Command::People { names: self.names }, ReqKind::People);
                self.request(Command::ListChats { limit: CHAT_LIMIT }, ReqKind::ListChats);
                // Self-heal the open chat after a reconnect.
                if let Some(View {
                    target: Target::Chat(id),
                    ..
                }) = &self.view
                {
                    let id = *id;
                    self.load_history(id, false);
                }
            }
            Event::NewMessage { message } => self.ingest(message, true, false, now),
            Event::MessageUpdated { message } => self.ingest(message, true, true, now),
            Event::Error { msg } => self.last_bridge_error = Some(msg),
        }
    }

    fn on_response(&mut self, r: Response, now: Instant) {
        let Some(kind) = self.inflight.remove(&r.id) else {
            return;
        };
        if let Some(err) = r.error {
            match kind {
                ReqKind::Send { local_id } => {
                    if let Some(p) = self.pending.iter_mut().find(|p| p.local_id == local_id) {
                        p.state = PendState::Failed(err);
                    }
                }
                ReqKind::History { .. } => {
                    if let Some(v) = &mut self.view {
                        v.loading = false;
                    }
                    self.flash_msg(err, now);
                }
                _ => self.flash_msg(err, now),
            }
            return;
        }
        match kind {
            ReqKind::ListChats => self.set_chats(r.chats.unwrap_or_default(), now),
            ReqKind::People => self.set_people(r.people.unwrap_or_default(), now),
            ReqKind::History { chat_id, older } => {
                self.on_history(chat_id, older, r.messages.unwrap_or_default())
            }
            ReqKind::Since => {
                for m in r.messages.unwrap_or_default() {
                    self.ingest(m, false, false, now);
                }
            }
            // Confirmation comes from the echo in chat.db, not from this response.
            ReqKind::Send { .. } => {}
        }
    }

    fn request(&mut self, cmd: Command, kind: ReqKind) -> bool {
        if self.status != Status::Connected {
            return false;
        }
        self.next_id += 1;
        let id = self.next_id;
        self.inflight.insert(id, kind);
        self.effects.push(Effect::Request(Request { id, cmd }));
        true
    }

    pub fn tick(&mut self, now: Instant) {
        if let Status::Disconnected { retry_at, .. } = &self.status
            && now >= *retry_at
        {
            self.status = Status::Connecting;
            self.effects.push(Effect::Reconnect);
        }
        for p in &mut self.pending {
            if p.state == PendState::Sending && now.duration_since(p.sent_at) > UNCONFIRMED_AFTER {
                p.state = PendState::Unconfirmed;
            }
        }
        if self
            .flash
            .as_ref()
            .is_some_and(|(_, at, dur)| now.duration_since(*at) > *dur)
        {
            self.flash = None;
        }
        if self.chats_dirty
            && !self
                .inflight
                .values()
                .any(|k| matches!(k, ReqKind::ListChats))
        {
            self.chats_dirty = false;
            // A new chat may be a new handle of a known person.
            self.request(Command::People { names: self.names }, ReqKind::People);
            self.request(Command::ListChats { limit: CHAT_LIMIT }, ReqKind::ListChats);
        }
    }

    fn flash_msg(&mut self, msg: impl Into<String>, now: Instant) {
        self.flash = Some((msg.into(), now, FLASH_FOR));
    }

    fn hint(&mut self, msg: impl Into<String>, now: Instant) {
        self.flash = Some((msg.into(), now, HINT_FOR));
    }

    // ---------------------------------------------------------------- data

    fn selected_chat_id(&self) -> Option<i64> {
        self.chats.get(self.selected).map(|c| c.chat_id)
    }

    /// Newest first; the cursor stays on the chat it was on.
    fn sort_chats(&mut self) {
        let sel = self.selected_chat_id();
        self.chats.sort_by(|a, b| {
            let key = |c: &Chat| c.last.as_ref().map(|m| (m.ts, m.rowid));
            key(b).cmp(&key(a)).then(a.chat_id.cmp(&b.chat_id))
        });
        self.select_id(sel);
    }

    /// Put the cursor on `id`, or on the top chat when it's gone (or `None`).
    fn select_id(&mut self, id: Option<i64>) {
        self.selected = id
            .and_then(|id| self.chats.iter().position(|c| c.chat_id == id))
            .unwrap_or(0);
    }

    fn open_chat_id(&self) -> Option<i64> {
        match &self.view {
            Some(View {
                target: Target::Chat(id),
                ..
            }) => Some(*id),
            _ => None,
        }
    }

    /// The chat row `raw` is shown under; itself when it isn't merged.
    fn primary_id(&self, raw: i64) -> i64 {
        self.chats
            .iter()
            .find(|c| c.chat_id == raw || c.chat_ids.contains(&raw))
            .map_or(raw, |c| c.chat_id)
    }

    fn person_of(&self, handle: &str) -> Option<usize> {
        self.people.iter().position(|p| {
            p.handles
                .iter()
                .any(|h| phone::same(h, handle, &self.country_code))
        })
    }

    /// Contacts name for a handle, when known.
    pub fn name_of(&self, handle: &str) -> Option<&str> {
        self.person_of(handle)
            .and_then(|p| self.people[p].name.as_deref())
    }

    /// Name if known, otherwise the handle itself.
    pub fn label(&self, handle: &str) -> String {
        self.name_of(handle)
            .map_or_else(|| handle.to_string(), String::from)
    }

    /// chat.db keeps one chat row per handle. Fold the rows of each configured
    /// person into one chat whose id and guid are those of the newest row.
    fn merge_chats(&self, chats: Vec<Chat>) -> Vec<Chat> {
        let mut out: Vec<Chat> = Vec::with_capacity(chats.len());
        let mut slot: HashMap<usize, usize> = HashMap::new();
        for c in chats {
            let person = match c.handles.as_slice() {
                [h] if !c.is_group => self.person_of(h),
                _ => None,
            };
            match person.and_then(|p| slot.get(&p).copied()) {
                Some(i) => merge_into(&mut out[i], c),
                None => {
                    if let Some(p) = person {
                        slot.insert(p, out.len());
                    }
                    out.push(c);
                }
            }
        }
        for (p, i) in slot {
            let c = &mut out[i];
            c.display = match &self.people[p].name {
                Some(name) => name.clone(),
                None => c.handles.join(" / "),
            };
        }
        // Groups without a name of their own are listed by participant; use names there too.
        for c in out.iter_mut().filter(|c| c.is_group) {
            if c.display == c.handles.join(", ") {
                c.display = c
                    .handles
                    .iter()
                    .map(|h| self.label(h))
                    .collect::<Vec<_>>()
                    .join(", ");
            }
        }
        out
    }

    fn set_people(&mut self, groups: Vec<PersonGroup>, now: Instant) {
        self.people = self.config_people.clone();
        self.people.extend(groups.into_iter().map(|g| Person {
            name: g.name,
            handles: g.handles,
        }));
        if self.loaded_once {
            let raw = self.raw_chats.clone();
            self.set_chats(raw, now);
        }
    }

    fn set_chats(&mut self, chats: Vec<Chat>, now: Instant) {
        self.raw_chats = chats.clone();
        let mut chats = self.merge_chats(chats);
        let open = self.open_chat_id();
        for c in &mut chats {
            let last = c.last.as_ref().map_or(0, |m| m.rowid);
            if open == Some(c.chat_id)
                || self
                    .read_upto
                    .get(&c.chat_id)
                    .is_some_and(|&upto| last <= upto)
            {
                c.unread = 0;
            }
            if !self.loaded_once {
                self.max_seen = self.max_seen.max(last);
            }
        }
        self.loaded_once = true;
        // Resolve the old selection only after sorting, so a first load (no
        // selection yet) lands on the newest chat rather than an arbitrary one.
        let sel = self.selected_chat_id();
        self.chats = chats;
        self.selected = 0;
        self.sort_chats();
        self.select_id(sel);
        // Echoes that landed in chats we hadn't listed yet can be matched now.
        for mut m in std::mem::take(&mut self.unmatched) {
            m.chat_id = self.primary_id(m.chat_id);
            self.match_pending(&m, true, now);
        }
    }

    fn load_history(&mut self, chat_id: i64, older: bool) {
        let before = if older {
            self.view
                .as_ref()
                .and_then(|v| v.messages.first())
                .map(|m| m.rowid)
        } else {
            None
        };
        let chat_ids = self
            .chats
            .iter()
            .find(|c| c.chat_id == chat_id)
            .map(|c| c.chat_ids.clone())
            .unwrap_or_default();
        let cmd = Command::History {
            chat_id,
            chat_ids,
            before_rowid: before,
            limit: HISTORY_PAGE,
        };
        if self.request(cmd, ReqKind::History { chat_id, older })
            && let Some(v) = &mut self.view
        {
            v.loading = true;
        }
    }

    fn on_history(&mut self, chat_id: i64, older: bool, mut msgs: Vec<Message>) {
        for m in &mut msgs {
            m.chat_id = self.primary_id(m.chat_id);
        }
        let Some(v) = &mut self.view else { return };
        if v.target != Target::Chat(chat_id) {
            return;
        }
        v.loading = false;
        v.no_more = msgs.len() < HISTORY_PAGE as usize;
        if older {
            let fresh: Vec<Message> = msgs
                .into_iter()
                .filter(|m| !v.messages.iter().any(|x| x.rowid == m.rowid))
                .collect();
            let added = fresh.len();
            v.messages.splice(0..0, fresh);
            if let Some(s) = &mut v.sel {
                *s += added;
            }
            if let Some(a) = &mut v.anchor {
                *a += added;
            }
        } else {
            // Keep live messages that arrived after this page was read.
            let newest = msgs.last().map_or(0, |m| m.rowid);
            let live: Vec<Message> = v.messages.drain(..).filter(|m| m.rowid > newest).collect();
            v.messages = msgs;
            v.messages.extend(live);
            v.appended = true;
        }
        v.messages.sort_by_key(|m| (m.ts, m.rowid));
        if let Some(last) = v.messages.last() {
            let e = self.read_upto.entry(chat_id).or_insert(0);
            *e = (*e).max(last.rowid);
        }
        self.clamp_sel();
    }

    fn ingest(&mut self, mut m: Message, live: bool, updated: bool, now: Instant) {
        if !updated {
            self.max_seen = self.max_seen.max(m.rowid);
        }
        let raw_chat = m.chat_id;
        m.chat_id = self.primary_id(raw_chat);
        let open_here = self.open_chat_id() == Some(m.chat_id);

        let mut chat_display = None;
        let known = if let Some(c) = self.chats.iter_mut().find(|c| c.chat_id == m.chat_id) {
            if updated {
                if c.last.as_ref().is_some_and(|l| l.rowid == m.rowid) {
                    c.last = Some(m.clone());
                }
            } else {
                if c.last
                    .as_ref()
                    .is_none_or(|l| (m.ts, m.rowid) >= (l.ts, l.rowid))
                {
                    c.last = Some(m.clone());
                }
                if open_here {
                    let e = self.read_upto.entry(m.chat_id).or_insert(0);
                    *e = (*e).max(m.rowid);
                } else if !m.from_me && m.kind != Kind::Tapback {
                    c.unread += 1;
                }
            }
            chat_display = Some(c.display.clone());
            true
        } else {
            false
        };
        if !known && !updated {
            self.chats_dirty = true;
        }
        if !updated {
            self.sort_chats();
        }

        if !updated && m.from_me && !self.match_pending(&m, known, now) && live {
            // Keep the raw id: the chat list refresh may merge it somewhere.
            let mut keep = m.clone();
            keep.chat_id = raw_chat;
            self.unmatched.push(keep);
            if self.unmatched.len() > UNMATCHED_KEEP {
                self.unmatched.remove(0);
            }
        }

        if let Some(v) = &mut self.view
            && v.target == Target::Chat(m.chat_id)
        {
            if let Some(existing) = v.messages.iter_mut().find(|x| x.rowid == m.rowid) {
                *existing = m.clone();
            } else if !updated {
                v.messages.push(m.clone());
                v.messages.sort_by_key(|x| (x.ts, x.rowid));
                v.appended = true;
            }
        }

        if live && !updated && !m.from_me && !open_here {
            if self.notifications {
                let sender = m
                    .sender
                    .as_deref()
                    .map(|s| self.label(s))
                    .unwrap_or_default();
                let title = match chat_display {
                    Some(d) if d != sender && !sender.is_empty() => format!("{sender} ({d})"),
                    Some(d) => d,
                    None => sender,
                };
                self.effects.push(Effect::Notify {
                    title: format!("imsg: {title}"),
                    body: preview(&m),
                });
            }
            if self.bell {
                self.effects.push(Effect::Bell);
            }
        }
    }

    /// Replace the oldest matching placeholder with its echo from chat.db.
    /// Returns whether a placeholder was confirmed.
    fn match_pending(&mut self, m: &Message, chat_known: bool, now: Instant) -> bool {
        let chat_handles = self
            .chats
            .iter()
            .find(|c| c.chat_id == m.chat_id)
            .map(|c| c.handles.clone());
        let cc = self.country_code.clone();
        let candidate = |p: &Pending| {
            matches!(p.state, PendState::Sending | PendState::Unconfirmed)
                && m.rowid > p.after_rowid
                && p.text.trim() == m.text.trim()
        };
        let direct = self.pending.iter().position(|p| {
            candidate(p)
                && match &p.target {
                    Target::Chat(id) => *id == m.chat_id,
                    Target::Draft(h) => {
                        !chat_known
                            || chat_handles
                                .as_ref()
                                .is_some_and(|hs| hs.iter().any(|x| phone::same(x, h, &cc)))
                    }
                }
        });
        let Some(pos) = direct else {
            return self.match_sibling(m, chat_known, candidate, now);
        };
        let p = self.pending.remove(pos);
        // A new conversation now has a real chat: switch the view to it.
        if let Target::Draft(h) = p.target
            && self
                .view
                .as_ref()
                .is_some_and(|v| v.target == Target::Draft(h.clone()))
        {
            self.view = Some(View::new(Target::Chat(m.chat_id)));
            self.load_history(m.chat_id, false);
        }
        self.clamp_sel();
        true
    }

    /// Messages files a copy under another 1:1 chat row of the same person
    /// when a contact has several handles. Count that as sent and say how to
    /// merge the rows.
    fn match_sibling(
        &mut self,
        m: &Message,
        chat_known: bool,
        candidate: impl Fn(&Pending) -> bool,
        now: Instant,
    ) -> bool {
        let one_on_one = |id: i64| self.chats.iter().any(|c| c.chat_id == id && !c.is_group);
        if !chat_known || !one_on_one(m.chat_id) {
            return false;
        }
        let Some(pos) = self.pending.iter().position(|p| {
            candidate(p)
                && matches!(&p.target, Target::Chat(id) if *id != m.chat_id && one_on_one(*id))
        }) else {
            return false;
        };
        let p = self.pending.remove(pos);
        let handles = |id: i64| {
            self.chats
                .iter()
                .find(|c| c.chat_id == id)
                .map(|c| c.handles.join(" / "))
                .unwrap_or_default()
        };
        let (from, to) = match p.target {
            Target::Chat(id) => (handles(id), handles(m.chat_id)),
            Target::Draft(_) => unreachable!(),
        };
        self.hint(
            format!(
                "sent, filed under {to}: put {from} and {to} on one Contacts card, \
                 or add [[person]] handles = [\"{from}\", \"{to}\"] to config.toml"
            ),
            now,
        );
        self.clamp_sel();
        true
    }

    pub fn items(&self) -> Vec<Item<'_>> {
        let Some(v) = &self.view else {
            return Vec::new();
        };
        let mut items: Vec<Item> = v.messages.iter().map(Item::Msg).collect();
        items.extend(
            self.pending
                .iter()
                .filter(|p| p.target == v.target)
                .map(Item::Pending),
        );
        items
    }

    fn clamp_sel(&mut self) {
        let n = self.items().len();
        if let Some(v) = &mut self.view {
            for idx in [&mut v.sel, &mut v.anchor] {
                if let Some(i) = idx {
                    if n == 0 {
                        *idx = None;
                    } else if *i >= n {
                        *i = n - 1;
                    }
                }
            }
        }
    }

    pub fn chat_for_view(&self) -> Option<&Chat> {
        self.open_chat_id()
            .and_then(|id| self.chats.iter().find(|c| c.chat_id == id))
    }

    // ---------------------------------------------------------------- actions

    fn open_selected(&mut self, now: Instant) {
        let Some(c) = self.chats.get_mut(self.selected) else {
            return;
        };
        c.unread = 0;
        let id = c.chat_id;
        let last = c.last.as_ref().map_or(0, |m| m.rowid);
        let e = self.read_upto.entry(id).or_insert(0);
        *e = (*e).max(last);
        self.focus = Focus::Input;
        if self.open_chat_id() == Some(id) {
            return;
        }
        self.view = Some(View::new(Target::Chat(id)));
        self.input.clear();
        self.cursor = 0;
        self.load_history(id, false);
        if self.status != Status::Connected {
            self.flash_msg("not connected", now);
        }
    }

    fn send_input(&mut self, now: Instant) {
        let text = self.input.trim_end().to_string();
        if text.trim().is_empty() {
            return;
        }
        let Some(target) = self.view.as_ref().map(|v| v.target.clone()) else {
            return;
        };
        let cmd = match &target {
            Target::Chat(id) => match self.chats.iter().find(|c| c.chat_id == *id) {
                Some(c) => Command::Send {
                    to: None,
                    chat_guid: Some(c.guid.clone()),
                    text: text.clone(),
                },
                None => return self.flash_msg("chat not loaded yet", now),
            },
            Target::Draft(h) => Command::Send {
                to: Some(h.clone()),
                chat_guid: None,
                text: text.clone(),
            },
        };
        self.next_local += 1;
        let local_id = self.next_local;
        if !self.request(cmd, ReqKind::Send { local_id }) {
            return self.flash_msg("not connected: message not sent", now);
        }
        self.pending.push(Pending {
            local_id,
            target,
            text,
            after_rowid: self.max_seen,
            sent_at: now,
            ts: chrono::Utc::now().timestamp(),
            state: PendState::Sending,
        });
        self.input.clear();
        self.cursor = 0;
        if let Some(v) = &mut self.view {
            v.scroll = 0;
            v.appended = true;
        }
    }

    fn submit_popup(&mut self, now: Instant) {
        let Some(p) = self.popup.take() else { return };
        let Some(handle) = phone::normalize(&p.to, &self.country_code) else {
            self.flash_msg(format!("not a phone number or email: {}", p.to.trim()), now);
            self.popup = Some(p);
            return;
        };
        let existing = self.chats.iter().position(|c| {
            !c.is_group
                && c.handles
                    .iter()
                    .any(|h| phone::same(h, &handle, &self.country_code))
        });
        match existing {
            Some(i) => {
                self.selected = i;
                self.open_selected(now);
            }
            None => {
                self.view = Some(View::new(Target::Draft(handle)));
                self.focus = Focus::Input;
            }
        }
        // Ready to send; Enter sends.
        self.input = p.msg;
        self.cursor = self.input.len();
    }

    fn scroll_by(&mut self, delta: isize) {
        let Some(v) = &mut self.view else { return };
        let max = v.max_scroll();
        v.scroll = (v.scroll as isize + delta).clamp(0, max as isize) as usize;
        let at_top = v.scroll >= max;
        let older = match &v.target {
            Target::Chat(id)
                if delta > 0 && at_top && !v.no_more && !v.loading && !v.messages.is_empty() =>
            {
                Some(*id)
            }
            _ => None,
        };
        if let Some(id) = older {
            self.load_history(id, true);
        }
    }

    fn page(&self) -> isize {
        self.view
            .as_ref()
            .map_or(10, |v| v.height.saturating_sub(2).max(1) as isize)
    }

    fn copy_selection(&mut self, with_meta: bool, now: Instant) {
        let Some(v) = &self.view else { return };
        let Some(sel) = v.sel else { return };
        let (a, b) = match v.anchor {
            Some(a) => (a.min(sel), a.max(sel)),
            None => (sel, sel),
        };
        let items = self.items();
        let lines: Vec<String> = items[a..=b.min(items.len() - 1)]
            .iter()
            .map(|it| {
                if with_meta {
                    item_with_meta(it, &|h| self.label(h))
                } else {
                    item_plain(it)
                }
            })
            .collect();
        let n = lines.len();
        self.effects.push(Effect::Copy(lines.join("\n")));
        if let Some(v) = &mut self.view {
            v.anchor = None;
        }
        self.flash_msg(
            format!("Copied {n} message{}", if n == 1 { "" } else { "s" }),
            now,
        );
    }

    // ---------------------------------------------------------------- input

    pub fn on_paste(&mut self, text: &str) {
        let text = text.replace("\r\n", "\n").replace('\r', "\n");
        if let Some(p) = &mut self.popup {
            if p.field == 0 {
                p.to.push_str(&text.replace('\n', " "));
            } else {
                p.msg.push_str(&text);
            }
            return;
        }
        if self.view.is_none() {
            return;
        }
        self.focus = Focus::Input;
        self.input.insert_str(self.cursor, &text);
        self.cursor += text.len();
    }

    pub fn on_key(&mut self, key: KeyEvent, now: Instant) {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        if ctrl && key.code == KeyCode::Char('c') {
            self.quit = true;
            return;
        }
        if self.popup.is_some() {
            return self.popup_key(key, now);
        }
        match key.code {
            KeyCode::Tab => return self.cycle_focus(true),
            KeyCode::BackTab => return self.cycle_focus(false),
            KeyCode::PageUp => return self.scroll_by(self.page()),
            KeyCode::PageDown => return self.scroll_by(-self.page()),
            _ => {}
        }
        match self.focus {
            Focus::List => self.list_key(key, now),
            Focus::Messages => self.messages_key(key, now),
            Focus::Input => self.input_key(key, now),
        }
    }

    fn cycle_focus(&mut self, forward: bool) {
        let order = if self.view.is_some() {
            vec![Focus::List, Focus::Messages, Focus::Input]
        } else {
            vec![Focus::List]
        };
        let i = order.iter().position(|f| *f == self.focus).unwrap_or(0);
        let n = order.len();
        self.set_focus(
            order[if forward {
                (i + 1) % n
            } else {
                (i + n - 1) % n
            }],
        );
    }

    fn set_focus(&mut self, f: Focus) {
        self.focus = f;
        if f == Focus::Messages {
            let n = self.items().len();
            if let Some(v) = &mut self.view
                && v.sel.is_none()
                && n > 0
            {
                v.sel = Some(n - 1);
                v.follow_sel = true;
            }
        }
    }

    fn global_key(&mut self, key: KeyEvent, now: Instant) -> bool {
        match key.code {
            KeyCode::Char('q') => self.quit = true,
            KeyCode::Char('n') => self.popup = Some(Popup::default()),
            KeyCode::Char('r') => match &mut self.status {
                Status::Disconnected { retry_at, .. } => *retry_at = now,
                _ => self.flash_msg("already connected", now),
            },
            _ => return false,
        }
        true
    }

    fn list_key(&mut self, key: KeyEvent, now: Instant) {
        let n = self.chats.len();
        match key.code {
            KeyCode::Char('j') | KeyCode::Down if n > 0 => {
                self.selected = (self.selected + 1).min(n - 1)
            }
            KeyCode::Char('k') | KeyCode::Up => self.selected = self.selected.saturating_sub(1),
            KeyCode::Char('g') | KeyCode::Home => self.selected = 0,
            KeyCode::Char('G') | KeyCode::End => self.selected = n.saturating_sub(1),
            KeyCode::Enter | KeyCode::Char('l') | KeyCode::Right => self.open_selected(now),
            _ => {
                self.global_key(key, now);
            }
        }
    }

    fn messages_key(&mut self, key: KeyEvent, now: Instant) {
        let n = self.items().len();
        match key.code {
            KeyCode::Char('j') | KeyCode::Down => {
                if let Some(v) = &mut self.view {
                    v.sel = Some(
                        v.sel
                            .map_or(n.saturating_sub(1), |s| (s + 1).min(n.saturating_sub(1))),
                    );
                    v.follow_sel = true;
                }
            }
            KeyCode::Char('k') | KeyCode::Up => {
                if let Some(v) = &mut self.view {
                    v.sel = Some(v.sel.map_or(n.saturating_sub(1), |s| s.saturating_sub(1)));
                    v.follow_sel = true;
                    // At the oldest loaded message: fetch more.
                    if v.sel == Some(0)
                        && let Target::Chat(id) = v.target
                        && !v.no_more
                        && !v.loading
                    {
                        self.load_history(id, true);
                    }
                }
            }
            KeyCode::Char('v') => {
                if let Some(v) = &mut self.view {
                    v.anchor = if v.anchor.is_some() { None } else { v.sel };
                }
            }
            KeyCode::Char('y') => self.copy_selection(false, now),
            KeyCode::Char('Y') => self.copy_selection(true, now),
            KeyCode::Esc => {
                if let Some(v) = &mut self.view
                    && v.anchor.is_some()
                {
                    v.anchor = None;
                } else {
                    self.focus = Focus::List;
                }
            }
            KeyCode::Char('i') | KeyCode::Enter => self.focus = Focus::Input,
            _ => {
                self.global_key(key, now);
            }
        }
    }

    fn input_key(&mut self, key: KeyEvent, now: Instant) {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let alt = key.modifiers.contains(KeyModifiers::ALT)
            || key.modifiers.contains(KeyModifiers::SHIFT);
        match key.code {
            KeyCode::Enter if alt => self.insert_char('\n'),
            KeyCode::Enter => self.send_input(now),
            KeyCode::Esc => self.focus = Focus::List,
            KeyCode::Char('u') if ctrl => {
                self.input.clear();
                self.cursor = 0;
            }
            KeyCode::Char('w') if ctrl => {
                let before = &self.input[..self.cursor];
                let trimmed = before.trim_end();
                let start = trimmed.rfind(char::is_whitespace).map_or(0, |i| i + 1);
                self.input.replace_range(start..self.cursor, "");
                self.cursor = start;
            }
            // Alt+letter is usually a fast Esc+letter, not text.
            KeyCode::Char(c) if !ctrl && !key.modifiers.contains(KeyModifiers::ALT) => {
                self.insert_char(c)
            }
            KeyCode::Backspace => {
                if let Some(c) = self.input[..self.cursor].chars().next_back() {
                    self.cursor -= c.len_utf8();
                    self.input.remove(self.cursor);
                }
            }
            KeyCode::Delete => {
                if self.cursor < self.input.len() {
                    self.input.remove(self.cursor);
                }
            }
            KeyCode::Left => {
                if let Some(c) = self.input[..self.cursor].chars().next_back() {
                    self.cursor -= c.len_utf8();
                }
            }
            KeyCode::Right => {
                if let Some(c) = self.input[self.cursor..].chars().next() {
                    self.cursor += c.len_utf8();
                }
            }
            KeyCode::Home => self.cursor = 0,
            KeyCode::End => self.cursor = self.input.len(),
            _ => {}
        }
    }

    fn insert_char(&mut self, c: char) {
        self.input.insert(self.cursor, c);
        self.cursor += c.len_utf8();
    }

    fn popup_key(&mut self, key: KeyEvent, now: Instant) {
        let Some(p) = &mut self.popup else { return };
        let field = if p.field == 0 { &mut p.to } else { &mut p.msg };
        match key.code {
            KeyCode::Esc => self.popup = None,
            KeyCode::Tab | KeyCode::BackTab | KeyCode::Up | KeyCode::Down => p.field = 1 - p.field,
            KeyCode::Enter if p.field == 0 => p.field = 1,
            KeyCode::Enter => self.submit_popup(now),
            KeyCode::Backspace => {
                field.pop();
            }
            KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => field.push(c),
            _ => {}
        }
    }
}

/// Fold chat row `b` into `a`; the newer row's id, guid and service win.
fn merge_into(a: &mut Chat, b: Chat) {
    let key = |m: &Option<Message>| m.as_ref().map(|m| (m.ts, m.rowid));
    let mut ids = vec![a.chat_id, b.chat_id];
    ids.append(&mut a.chat_ids);
    ids.extend(b.chat_ids.iter().copied());
    if key(&b.last) > key(&a.last) {
        a.chat_id = b.chat_id;
        a.guid = b.guid;
        a.service = b.service;
        a.last = b.last;
    }
    ids.retain(|id| *id != a.chat_id);
    ids.sort_unstable();
    ids.dedup();
    a.chat_ids = ids;
    for h in b.handles {
        if !a.handles.contains(&h) {
            a.handles.push(h);
        }
    }
    a.unread += b.unread;
}

// -------------------------------------------------------------------- text helpers

pub fn attachment_label(a: &proto::Attachment) -> String {
    let kind = match a.mime.as_deref().and_then(|m| m.split('/').next()) {
        Some("image") => "image",
        Some("video") => "video",
        Some("audio") => "audio",
        _ => "file",
    };
    format!("[{kind}: {}]", a.name)
}

/// One-line summary for the chat list and notifications.
pub fn preview(m: &Message) -> String {
    if m.unsent {
        return if m.from_me {
            "⊘ you unsent a message".into()
        } else {
            "⊘ unsent a message".into()
        };
    }
    let body = match (&m.tapback, m.kind) {
        (Some(t), _) => format!("{} to “{}”", t.emoji, t.target_text),
        (_, Kind::Attachment) => m
            .attachments
            .iter()
            .map(attachment_label)
            .collect::<Vec<_>>()
            .join(" "),
        _ => m.text.lines().next().unwrap_or("").to_string(),
    };
    if m.from_me {
        format!("you: {body}")
    } else {
        body
    }
}

pub fn item_plain(it: &Item) -> String {
    match it {
        Item::Pending(p) => p.text.clone(),
        Item::Msg(m) if m.unsent => "(unsent)".into(),
        Item::Msg(m) => match &m.tapback {
            Some(t) => format!("{} to “{}”", t.emoji, t.target_text),
            None => {
                let mut parts: Vec<String> = Vec::new();
                if !m.text.is_empty() {
                    parts.push(m.text.clone());
                }
                parts.extend(m.attachments.iter().map(attachment_label));
                parts.join("\n")
            }
        },
    }
}

pub fn item_with_meta(it: &Item, label: &dyn Fn(&str) -> String) -> String {
    let (ts, who) = match it {
        Item::Pending(p) => (p.ts, "me".to_string()),
        Item::Msg(m) => (
            m.ts,
            if m.from_me {
                "me".into()
            } else {
                m.sender.as_deref().map(label).unwrap_or_default()
            },
        ),
    };
    let time = Local
        .timestamp_opt(ts, 0)
        .single()
        .map(|t| t.format("%H:%M").to_string())
        .unwrap_or_default();
    format!("[{time} {who}] {}", item_plain(it))
}

#[cfg(test)]
mod tests {
    use super::*;
    use proto::Tapback;
    use ratatui::crossterm::event::KeyEventKind;

    fn cfg() -> Config {
        Config {
            host: "mac".into(),
            notifications: true,
            ..Default::default()
        }
    }

    fn msg(rowid: i64, chat_id: i64, from_me: bool, text: &str) -> Message {
        Message {
            rowid,
            chat_id,
            sender: (!from_me).then(|| "+15550000001".to_string()),
            from_me,
            text: text.into(),
            ts: 1_791_369_060 + rowid,
            service: "iMessage".into(),
            is_error: false,
            edited: false,
            unsent: false,
            kind: Kind::Text,
            attachments: vec![],
            tapback: None,
        }
    }

    fn chat(id: i64, handle: &str, last: Message) -> Chat {
        Chat {
            chat_id: id,
            guid: format!("iMessage;-;{handle}"),
            display: handle.into(),
            is_group: false,
            service: "iMessage".into(),
            handles: vec![handle.into()],
            unread: 0,
            last: Some(last),
            chat_ids: vec![],
        }
    }

    fn key(c: KeyCode) -> KeyEvent {
        KeyEvent::new_with_kind(c, KeyModifiers::NONE, KeyEventKind::Press)
    }

    fn line<T: serde::Serialize>(v: &T) -> String {
        serde_json::to_string(v).unwrap()
    }

    /// Pops queued requests, returning (id, cmd).
    fn requests(app: &mut App) -> Vec<(u64, Command)> {
        let mut out = Vec::new();
        app.effects.retain(|e| match e {
            Effect::Request(r) => {
                out.push((r.id, r.cmd.clone()));
                false
            }
            _ => true,
        });
        out
    }

    fn connected() -> (App, Instant) {
        connected_with(&cfg())
    }

    fn connected_with(cfg: &Config) -> (App, Instant) {
        let now = Instant::now();
        let mut app = App::new(cfg);
        app.on_line(
            &line(&Event::Hello {
                version: PROTOCOL_VERSION,
                bridge: "t".into(),
            }),
            now,
        );
        let reqs = requests(&mut app);
        assert!(matches!(
            reqs[..],
            [(_, Command::People { .. }), (_, Command::ListChats { .. })]
        ));
        let chats = vec![
            chat(1, "+15550000001", msg(10, 1, false, "hi")),
            chat(2, "+15550000002", msg(5, 2, false, "yo")),
        ];
        app.on_line(
            &line(&Response {
                id: list_id(&reqs),
                chats: Some(chats),
                ..Default::default()
            }),
            now,
        );
        (app, now)
    }

    #[test]
    fn version_mismatch_is_fatal() {
        let mut app = App::new(&cfg());
        app.on_line(
            &line(&Event::Hello {
                version: 99,
                bridge: "9.9".into(),
            }),
            Instant::now(),
        );
        assert!(matches!(&app.status, Status::Fatal(m) if m.contains("rebuild")));
        assert!(app.effects.iter().any(|e| matches!(e, Effect::Disconnect)));
        app.on_closed("x".into(), Instant::now());
        assert!(
            matches!(app.status, Status::Fatal(_)),
            "no reconnect after fatal"
        );
    }

    #[test]
    fn send_echo_and_duplicate_text() {
        let (mut app, now) = connected();
        app.on_key(key(KeyCode::Enter), now); // open chat 1
        let reqs = requests(&mut app);
        assert!(matches!(
            reqs[..],
            [(_, Command::History { chat_id: 1, .. })]
        ));
        app.on_line(
            &line(&Response {
                id: reqs[0].0,
                messages: Some(vec![msg(10, 1, false, "hi")]),
                ..Default::default()
            }),
            now,
        );

        for _ in 0..2 {
            app.on_paste("ok");
            app.on_key(key(KeyCode::Enter), now);
        }
        let sends = requests(&mut app);
        assert_eq!(sends.len(), 2);
        assert!(
            matches!(&sends[0].1, Command::Send { chat_guid: Some(g), .. } if g == "iMessage;-;+15550000001")
        );
        assert_eq!(app.pending.len(), 2);
        assert_eq!(app.items().len(), 3);

        // An older "ok" (rowid <= after_rowid) must not match.
        app.on_line(
            &line(&Event::NewMessage {
                message: msg(11, 1, true, "ok"),
            }),
            now,
        );
        assert_eq!(app.pending.len(), 1, "first placeholder replaced");
        app.on_line(
            &line(&Event::NewMessage {
                message: msg(12, 1, true, "ok"),
            }),
            now,
        );
        assert!(app.pending.is_empty());
        assert_eq!(app.items().len(), 3);
        assert!(
            app.effects
                .iter()
                .all(|e| !matches!(e, Effect::Notify { .. })),
            "own messages don't notify"
        );
    }

    #[test]
    fn failed_send_and_unconfirmed() {
        let (mut app, now) = connected();
        app.on_key(key(KeyCode::Enter), now);
        requests(&mut app);
        app.on_paste("one");
        app.on_key(key(KeyCode::Enter), now);
        let id = requests(&mut app)[0].0;
        app.on_line(&line(&Response::error(id, "osascript failed")), now);
        assert_eq!(
            app.pending[0].state,
            PendState::Failed("osascript failed".into())
        );

        app.on_paste("two");
        app.on_key(key(KeyCode::Enter), now);
        app.tick(now + Duration::from_secs(31));
        assert_eq!(app.pending[1].state, PendState::Unconfirmed);
    }

    #[test]
    fn notifications_unread_and_updates() {
        let (mut app, now) = connected();
        app.on_line(
            &line(&Event::NewMessage {
                message: msg(20, 2, false, "new in 2"),
            }),
            now,
        );
        assert_eq!(app.chats[0].chat_id, 2, "chat bumped to top");
        assert_eq!(app.chats[0].unread, 1);
        assert!(
            app.effects
                .iter()
                .any(|e| matches!(e, Effect::Notify { body, .. } if body == "new in 2"))
        );
        app.effects.clear();

        let mut edited = msg(20, 2, false, "edited text");
        edited.edited = true;
        app.on_line(&line(&Event::MessageUpdated { message: edited }), now);
        assert!(app.effects.is_empty(), "updates never notify");
        assert_eq!(app.chats[0].last.as_ref().unwrap().text, "edited text");
        assert_eq!(app.chats[0].unread, 1, "updates don't bump unread");

        // Unknown chat triggers a list refresh on the next tick.
        app.on_line(
            &line(&Event::NewMessage {
                message: msg(21, 99, false, "who dis"),
            }),
            now,
        );
        app.effects.clear();
        app.tick(now);
        assert!(matches!(
            requests(&mut app)[..],
            [(_, Command::People { .. }), (_, Command::ListChats { .. })]
        ));
    }

    #[test]
    fn reconnect_catch_up_is_silent() {
        let (mut app, now) = connected();
        app.on_closed("Connection reset".into(), now);
        assert!(
            matches!(&app.status, Status::Disconnected { reason, .. } if reason == "Connection reset")
        );
        app.tick(now + Duration::from_millis(500));
        assert!(app.effects.is_empty());
        app.tick(now + Duration::from_secs(1));
        assert!(matches!(app.effects[..], [Effect::Reconnect]));
        app.effects.clear();

        app.on_line(
            &line(&Event::Hello {
                version: PROTOCOL_VERSION,
                bridge: "t".into(),
            }),
            now,
        );
        let reqs = requests(&mut app);
        let (since_id, Command::Since { rowid: 10 }) = reqs[0] else {
            panic!("{reqs:?}")
        };
        app.on_line(
            &line(&Response {
                id: since_id,
                messages: Some(vec![msg(30, 2, false, "missed")]),
                ..Default::default()
            }),
            now,
        );
        assert!(
            app.effects
                .iter()
                .all(|e| !matches!(e, Effect::Notify { .. }))
        );
        assert_eq!(app.chats[0].chat_id, 2);
    }

    #[test]
    fn bridge_error_becomes_reason_and_backoff_grows() {
        let now = Instant::now();
        let mut app = App::new(&cfg());
        app.on_line(
            &line(&Event::Error {
                msg: "bridge service not running".into(),
            }),
            now,
        );
        app.on_closed("exit 1".into(), now);
        assert!(
            matches!(&app.status, Status::Disconnected { reason, .. } if reason == "bridge service not running")
        );
        app.on_closed("again".into(), now);
        let Status::Disconnected { retry_at, .. } = app.status else {
            panic!()
        };
        assert_eq!(retry_at - now, Duration::from_secs(2));
    }

    #[test]
    fn new_conversation_matches_existing_chat_or_drafts() {
        let (mut app, now) = connected();
        app.on_key(key(KeyCode::Char('n')), now);
        app.on_paste("(555) 000-0002");
        app.on_key(key(KeyCode::Enter), now);
        app.on_paste("hello");
        app.on_key(key(KeyCode::Enter), now);
        assert_eq!(app.view.as_ref().unwrap().target, Target::Chat(2));
        assert_eq!(app.input, "hello");
        assert_eq!(app.focus, Focus::Input);

        app.on_key(key(KeyCode::Char('n')), now); // typed into the input, not a popup
        assert!(app.popup.is_none());
        app.on_key(key(KeyCode::Esc), now);
        app.on_key(key(KeyCode::Char('n')), now);
        app.on_paste("555 000 0009");
        app.on_key(key(KeyCode::Enter), now);
        app.on_key(key(KeyCode::Enter), now);
        assert_eq!(
            app.view.as_ref().unwrap().target,
            Target::Draft("+15550000009".into())
        );
        requests(&mut app);
        app.on_paste("first!");
        app.on_key(key(KeyCode::Enter), now);
        assert!(
            matches!(&requests(&mut app)[..], [(_, Command::Send { to: Some(t), .. })] if t == "+15550000009")
        );

        // Echo arrives in a brand-new chat: the view follows it.
        app.on_line(
            &line(&Event::NewMessage {
                message: msg(40, 7, true, "first!"),
            }),
            now,
        );
        assert_eq!(app.view.as_ref().unwrap().target, Target::Chat(7));
        assert!(app.pending.is_empty());
    }

    #[test]
    fn copy_selection_and_range() {
        let (mut app, now) = connected();
        app.on_key(key(KeyCode::Enter), now);
        let id = requests(&mut app)[0].0;
        let mut tap = msg(12, 1, true, "");
        tap.kind = Kind::Tapback;
        tap.tapback = Some(Tapback {
            emoji: "❤️".into(),
            target_text: "b".into(),
        });
        let history = vec![msg(10, 1, false, "a"), msg(11, 1, false, "b"), tap];
        app.on_line(
            &line(&Response {
                id,
                messages: Some(history),
                ..Default::default()
            }),
            now,
        );

        app.on_key(key(KeyCode::Tab), now); // Input -> List
        app.on_key(key(KeyCode::Tab), now); // List -> Messages, selects last
        assert_eq!(app.focus, Focus::Messages);
        app.on_key(key(KeyCode::Char('y')), now);
        assert!(matches!(app.effects.last(), Some(Effect::Copy(t)) if t == "❤️ to “b”"));
        app.on_key(key(KeyCode::Char('k')), now);
        app.on_key(key(KeyCode::Char('v')), now);
        app.on_key(key(KeyCode::Char('k')), now);
        app.on_key(key(KeyCode::Char('y')), now);
        assert!(matches!(app.effects.last(), Some(Effect::Copy(t)) if t == "a\nb"));
        assert_eq!(app.flash.as_ref().unwrap().0, "Copied 2 messages");
    }

    fn two_handle_person() -> Config {
        Config {
            people: vec![Person {
                name: None,
                handles: vec!["(555) 000-0001".into(), "friend@example.com".into()],
            }],
            ..cfg()
        }
    }

    /// Id of the pending list_chats request.
    fn list_id(reqs: &[(u64, Command)]) -> u64 {
        reqs.iter()
            .find(|(_, c)| matches!(c, Command::ListChats { .. }))
            .map(|(id, _)| *id)
            .expect("list_chats requested")
    }

    fn people_id(reqs: &[(u64, Command)]) -> u64 {
        reqs.iter()
            .find(|(_, c)| matches!(c, Command::People { .. }))
            .map(|(id, _)| *id)
            .expect("people requested")
    }

    /// Answers the pending list_chats; returns the pending people request's id.
    fn three_chats(app: &mut App, now: Instant) -> u64 {
        let reqs = requests(app);
        let chats = vec![
            chat(1, "+15550000001", msg(10, 1, false, "by phone")),
            chat(2, "+15550000002", msg(5, 2, false, "yo")),
            chat(3, "friend@example.com", msg(12, 3, false, "by email")),
        ];
        app.on_line(
            &line(&Response {
                id: list_id(&reqs),
                chats: Some(chats),
                ..Default::default()
            }),
            now,
        );
        people_id(&reqs)
    }

    #[test]
    fn contacts_groups_merge_chats() {
        let now = Instant::now();
        let mut app = App::new(&cfg());
        app.on_line(
            &line(&Event::Hello {
                version: PROTOCOL_VERSION,
                bridge: "t".into(),
            }),
            now,
        );
        let people = three_chats(&mut app, now);
        assert_eq!(app.chats.len(), 3, "nothing merged before Contacts answers");
        assert_eq!(app.selected, 0, "first load selects the newest chat");
        app.on_key(key(KeyCode::Enter), now); // open the newest: email row (3)
        requests(&mut app);

        let group = PersonGroup {
            handles: vec!["+15550000001".into(), "friend@example.com".into()],
            name: None,
        };
        app.on_line(
            &line(&Response {
                id: people,
                people: Some(vec![group]),
                ..Default::default()
            }),
            now,
        );
        assert_eq!(app.chats.len(), 2);
        let merged = &app.chats[0];
        assert_eq!((merged.chat_id, merged.chat_ids.clone()), (3, vec![1]));
        assert_eq!(merged.display, "+15550000001 / friend@example.com");
        assert_eq!(
            app.view.as_ref().unwrap().target,
            Target::Chat(3),
            "open chat survives the re-merge"
        );

        // The copy of a send filed under the phone row confirms, with no hint.
        app.on_paste("hi");
        app.on_key(key(KeyCode::Enter), now);
        requests(&mut app);
        app.on_line(
            &line(&Event::NewMessage {
                message: msg(20, 1, true, "hi"),
            }),
            now,
        );
        assert!(app.pending.is_empty());
        assert!(app.flash.is_none());

        // A new chat triggers both refreshes; a later answer with a name is used.
        app.on_line(
            &line(&Event::NewMessage {
                message: msg(21, 8, false, "new row"),
            }),
            now,
        );
        app.tick(now);
        let reqs = requests(&mut app);
        assert_eq!(reqs.len(), 2);
        let named = PersonGroup {
            handles: vec!["+15550000001".into(), "friend@example.com".into()],
            name: Some("Alex".into()),
        };
        app.on_line(
            &line(&Response {
                id: people_id(&reqs),
                people: Some(vec![named]),
                ..Default::default()
            }),
            now,
        );
        assert_eq!(app.chats[0].display, "Alex");
    }

    #[test]
    fn names_label_group_senders_and_copies() {
        let now = Instant::now();
        let mut app = App::new(&cfg());
        app.on_line(
            &line(&Event::Hello {
                version: PROTOCOL_VERSION,
                bridge: "t".into(),
            }),
            now,
        );
        let reqs = requests(&mut app);
        let mut group = chat(5, "+15550000001", msg(3, 5, false, "hey"));
        group.is_group = true;
        group.handles.push("+15550000002".into());
        group.display = "+15550000001, +15550000002".into();
        let mut named_group = chat(6, "+15550000001", msg(2, 6, false, "x"));
        named_group.is_group = true;
        named_group.display = "Weekend Crew".into();
        app.on_line(
            &line(&Response {
                id: list_id(&reqs),
                chats: Some(vec![group, named_group]),
                ..Default::default()
            }),
            now,
        );
        app.on_line(
            &line(&Response {
                id: people_id(&reqs),
                people: Some(vec![PersonGroup {
                    handles: vec!["+15550000001".into()],
                    name: Some("Alex".into()),
                }]),
                ..Default::default()
            }),
            now,
        );
        assert_eq!(app.chats[0].display, "Alex, +15550000002");
        assert_eq!(
            app.chats[1].display, "Weekend Crew",
            "a group's own name wins"
        );
        assert_eq!(app.label("+15550000001"), "Alex");
        assert_eq!(app.label("+15550000009"), "+15550000009");

        app.on_key(key(KeyCode::Enter), now);
        let id = requests(&mut app)[0].0;
        app.on_line(
            &line(&Response {
                id,
                messages: Some(vec![msg(3, 5, false, "hey")]),
                ..Default::default()
            }),
            now,
        );
        app.on_key(key(KeyCode::Tab), now);
        app.on_key(key(KeyCode::Tab), now);
        app.on_key(key(KeyCode::Char('Y')), now);
        assert!(matches!(app.effects.last(), Some(Effect::Copy(t)) if t.ends_with(" Alex] hey")));

        app.on_line(
            &line(&Event::NewMessage {
                message: msg(9, 6, false, "ping"),
            }),
            now,
        );
        assert!(app.effects.iter().any(
            |e| matches!(e, Effect::Notify { title, .. } if title == "imsg: Alex (Weekend Crew)")
        ));
    }

    #[test]
    fn names_flag_is_sent() {
        let now = Instant::now();
        let mut app = App::new(&Config {
            names: true,
            ..cfg()
        });
        app.on_line(
            &line(&Event::Hello {
                version: PROTOCOL_VERSION,
                bridge: "t".into(),
            }),
            now,
        );
        let reqs = requests(&mut app);
        assert!(matches!(reqs[0].1, Command::People { names: true }));
    }

    #[test]
    fn configured_person_merges_chat_rows() {
        let now = Instant::now();
        let mut app = App::new(&two_handle_person());
        app.on_line(
            &line(&Event::Hello {
                version: PROTOCOL_VERSION,
                bridge: "t".into(),
            }),
            now,
        );
        three_chats(&mut app, now);

        assert_eq!(app.chats.len(), 2);
        let merged = &app.chats[0];
        assert_eq!(
            (merged.chat_id, merged.chat_ids.clone()),
            (3, vec![1]),
            "newest row is primary"
        );
        assert_eq!(merged.display, "+15550000001 / friend@example.com");
        assert_eq!(merged.guid, "iMessage;-;friend@example.com");

        app.on_key(key(KeyCode::Enter), now);
        let reqs = requests(&mut app);
        assert!(
            matches!(&reqs[0].1, Command::History { chat_id: 3, chat_ids, .. } if chat_ids == &[1])
        );
        app.on_line(
            &line(&Response {
                id: reqs[0].0,
                messages: Some(vec![
                    msg(10, 1, false, "by phone"),
                    msg(12, 3, false, "by email"),
                ]),
                ..Default::default()
            }),
            now,
        );
        assert_eq!(app.items().len(), 2);
        assert!(
            app.view
                .as_ref()
                .unwrap()
                .messages
                .iter()
                .all(|m| m.chat_id == 3),
            "history rows map to the primary"
        );

        // The copy of a send is filed under the phone row: still confirmed, and shown.
        app.on_paste("hi");
        app.on_key(key(KeyCode::Enter), now);
        assert!(
            matches!(&requests(&mut app)[0].1, Command::Send { chat_guid: Some(g), .. } if g == "iMessage;-;friend@example.com")
        );
        app.on_line(
            &line(&Event::NewMessage {
                message: msg(20, 1, true, "hi"),
            }),
            now,
        );
        assert!(app.pending.is_empty());
        assert_eq!(app.items().len(), 3);
        assert!(app.flash.is_none());

        // Incoming on the phone row while the merged chat is open: no unread, no notification.
        app.on_line(
            &line(&Event::NewMessage {
                message: msg(21, 1, false, "back"),
            }),
            now,
        );
        assert_eq!(app.chats[0].unread, 0);
        assert!(
            app.effects
                .iter()
                .all(|e| !matches!(e, Effect::Notify { .. }))
        );
        assert_eq!(app.items().len(), 4);

        // Typing either handle in the popup opens the merged chat.
        app.on_key(key(KeyCode::Esc), now);
        app.on_key(key(KeyCode::Char('n')), now);
        app.on_paste("friend@example.com");
        app.on_key(key(KeyCode::Enter), now);
        app.on_key(key(KeyCode::Enter), now);
        assert_eq!(app.view.as_ref().unwrap().target, Target::Chat(3));
    }

    #[test]
    fn named_person_without_merge_shows_name() {
        let now = Instant::now();
        let cfg = Config {
            people: vec![Person {
                name: Some("Alex".into()),
                handles: vec!["+15550000002".into()],
            }],
            ..cfg()
        };
        let mut app = App::new(&cfg);
        app.on_line(
            &line(&Event::Hello {
                version: PROTOCOL_VERSION,
                bridge: "t".into(),
            }),
            now,
        );
        three_chats(&mut app, now);
        assert_eq!(app.chats.len(), 3);
        assert_eq!(
            app.chats.iter().find(|c| c.chat_id == 2).unwrap().display,
            "Alex"
        );
    }

    #[test]
    fn sibling_echo_confirms_and_hints() {
        let now = Instant::now();
        let mut app = App::new(&cfg());
        app.on_line(
            &line(&Event::Hello {
                version: PROTOCOL_VERSION,
                bridge: "t".into(),
            }),
            now,
        );
        three_chats(&mut app, now);
        // Open the phone chat (chat 1).
        app.selected = app.chats.iter().position(|c| c.chat_id == 1).unwrap();
        app.on_key(key(KeyCode::Enter), now);
        requests(&mut app);
        app.on_paste("hello");
        app.on_key(key(KeyCode::Enter), now);
        requests(&mut app);
        app.on_line(
            &line(&Event::NewMessage {
                message: msg(20, 3, true, "hello"),
            }),
            now,
        );
        assert!(
            app.pending.is_empty(),
            "copy under the email row counts as sent"
        );
        let hint = &app.flash.as_ref().unwrap().0;
        assert!(
            hint.contains("[[person]]")
                && hint.contains("+15550000001")
                && hint.contains("friend@example.com"),
            "{hint}"
        );

        // A group chat never counts as a sibling.
        app.on_paste("again");
        app.on_key(key(KeyCode::Enter), now);
        let mut group = chat(7, "+15550000005", msg(1, 7, false, "x"));
        group.is_group = true;
        group.handles.push("+15550000006".into());
        app.chats.push(group);
        app.on_line(
            &line(&Event::NewMessage {
                message: msg(21, 7, true, "again"),
            }),
            now,
        );
        assert_eq!(app.pending.len(), 1);
    }

    #[test]
    fn echo_in_unlisted_chat_is_retried_after_refresh() {
        let (mut app, now) = connected();
        app.on_key(key(KeyCode::Enter), now); // chat 1
        requests(&mut app);
        app.on_paste("yo");
        app.on_key(key(KeyCode::Enter), now);
        requests(&mut app);
        // Messages created a new row for the same person (e.g. an SMS row).
        app.on_line(
            &line(&Event::NewMessage {
                message: msg(30, 9, true, "yo"),
            }),
            now,
        );
        assert_eq!(app.pending.len(), 1);
        app.tick(now);
        let reqs = requests(&mut app);
        assert_eq!(reqs.len(), 2);
        let mut sms = chat(9, "+15550000001", msg(30, 9, true, "yo"));
        sms.guid = "SMS;-;+15550000001".into();
        sms.service = "SMS".into();
        let chats = vec![chat(1, "+15550000001", msg(10, 1, false, "hi")), sms];
        app.on_line(
            &line(&Response {
                id: list_id(&reqs),
                chats: Some(chats),
                ..Default::default()
            }),
            now,
        );
        assert!(app.pending.is_empty());
        assert!(app.flash.as_ref().unwrap().0.contains("[[person]]"));
    }

    #[test]
    fn input_editing() {
        let (mut app, now) = connected();
        app.on_key(key(KeyCode::Enter), now);
        for c in "héllo wörld".chars() {
            app.on_key(key(KeyCode::Char(c)), now);
        }
        app.on_key(key(KeyCode::Left), now);
        app.on_key(key(KeyCode::Backspace), now);
        assert_eq!(app.input, "héllo wörd");
        let mut alt_enter = key(KeyCode::Enter);
        alt_enter.modifiers = KeyModifiers::ALT;
        app.on_key(alt_enter, now);
        assert_eq!(app.input, "héllo wör\nd");
        app.on_key(
            KeyEvent {
                modifiers: KeyModifiers::CONTROL,
                ..key(KeyCode::Char('w'))
            },
            now,
        );
        assert_eq!(app.input, "héllo d");
        app.on_paste("a\r\nb");
        assert_eq!(app.input, "héllo a\nbd");
    }
}
