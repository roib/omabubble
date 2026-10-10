-- Fake Messages database for development and tests. All numbers and text are made up.
-- Columns mirror the parts of the macOS 15 chat.db schema that the bridge reads.
-- Build: sqlite3 fixture.db < make_fixture.sql

CREATE TABLE handle (
    ROWID INTEGER PRIMARY KEY AUTOINCREMENT UNIQUE,
    id TEXT NOT NULL,
    country TEXT,
    service TEXT NOT NULL,
    uncanonicalized_id TEXT,
    person_centric_id TEXT
);

CREATE TABLE chat (
    ROWID INTEGER PRIMARY KEY AUTOINCREMENT,
    guid TEXT UNIQUE NOT NULL,
    style INTEGER,
    state INTEGER,
    account_id TEXT,
    chat_identifier TEXT,
    service_name TEXT,
    room_name TEXT,
    display_name TEXT,
    last_read_message_timestamp INTEGER DEFAULT 0
);

CREATE TABLE message (
    ROWID INTEGER PRIMARY KEY AUTOINCREMENT,
    guid TEXT UNIQUE NOT NULL,
    text TEXT,
    handle_id INTEGER DEFAULT 0,
    service TEXT,
    error INTEGER DEFAULT 0,
    date INTEGER,
    date_read INTEGER,
    date_delivered INTEGER,
    is_from_me INTEGER DEFAULT 0,
    is_read INTEGER DEFAULT 0,
    cache_has_attachments INTEGER DEFAULT 0,
    item_type INTEGER DEFAULT 0,
    associated_message_guid TEXT DEFAULT NULL,
    associated_message_type INTEGER DEFAULT 0,
    associated_message_emoji TEXT DEFAULT NULL,
    attributedBody BLOB,
    date_edited INTEGER DEFAULT 0,
    date_retracted INTEGER DEFAULT 0
);

CREATE TABLE chat_handle_join (chat_id INTEGER, handle_id INTEGER, UNIQUE(chat_id, handle_id));
CREATE TABLE chat_message_join (chat_id INTEGER, message_id INTEGER, message_date INTEGER DEFAULT 0,
    PRIMARY KEY (chat_id, message_id));
CREATE INDEX chat_message_join_idx_message_date_id_chat_id ON chat_message_join(chat_id, message_date, message_id);

CREATE TABLE attachment (
    ROWID INTEGER PRIMARY KEY AUTOINCREMENT,
    guid TEXT UNIQUE NOT NULL,
    filename TEXT,
    mime_type TEXT,
    transfer_name TEXT,
    total_bytes INTEGER DEFAULT 0
);
CREATE TABLE message_attachment_join (message_id INTEGER, attachment_id INTEGER, UNIQUE(message_id, attachment_id));


INSERT INTO handle (ROWID, id, service) VALUES
    (1, '+15550000001', 'iMessage'),
    (2, '+15550000002', 'iMessage'),
    (3, '+15550000003', 'SMS'),
    (4, 'friend@example.com', 'iMessage'),
    (5, '+15550000004', 'iMessage');

INSERT INTO chat (ROWID, guid, style, chat_identifier, service_name, display_name) VALUES
    (1, 'iMessage;-;+15550000001', 45, '+15550000001', 'iMessage', ''),
    (2, 'SMS;-;+15550000003', 45, '+15550000003', 'SMS', ''),
    (3, 'iMessage;+;chat000000000000000001', 43, 'chat000000000000000001', 'iMessage', ''),
    (4, 'iMessage;-;friend@example.com', 45, 'friend@example.com', 'iMessage', NULL),
    (5, 'iMessage;+;chat000000000000000002', 43, 'chat000000000000000002', 'iMessage', 'Weekend Crew');

INSERT INTO chat_handle_join VALUES (1,1), (2,3), (3,2), (3,5), (4,4), (5,1), (5,2);

-- Dates are Apple-epoch nanoseconds: (unix seconds - 978307200) * 1e9.
INSERT INTO message (ROWID, guid, text, handle_id, service, error, date, is_from_me, is_read,
                     cache_has_attachments, item_type, associated_message_guid, associated_message_type,
                     attributedBody, date_edited, date_retracted) VALUES
    -- chat 1: 1:1 iMessage
    (1, 'G1', 'yo did you get the tickets?', 1, 'iMessage', 0, (strftime('%s','2026-10-07 10:31:00') - 978307200) * 1000000000, 0, 1, 0, 0, NULL, 0, NULL, 0, 0),
    (2, 'G2', 'yeah just bought them, sending now', 0, 'iMessage', 0, (strftime('%s','2026-10-07 10:33:00') - 978307200) * 1000000000, 1, 1, 0, 0, NULL, 0, NULL, 0, 0),
    (3, 'G3', 'nice 🙌', 1, 'iMessage', 0, (strftime('%s','2026-10-07 10:35:00') - 978307200) * 1000000000, 0, 1, 0, 0, NULL, 0, NULL, 0, 0),
    (4, 'G4', 'what time should we leave', 1, 'iMessage', 0, (strftime('%s','2026-10-07 10:35:30') - 978307200) * 1000000000, 0, 1, 0, 0, NULL, 0, NULL, 0, 0),
    (5, 'G5', 'Loved “what time should we leave”', 0, 'iMessage', 0, (strftime('%s','2026-10-07 10:36:00') - 978307200) * 1000000000, 1, 1, 0, 0, 'p:0/G4', 2000, NULL, 0, 0),
    (6, 'G6', 'around 6? traffic will be bad', 0, 'iMessage', 0, (strftime('%s','2026-10-07 10:40:00') - 978307200) * 1000000000, 1, 1, 0, 0, NULL, 0, NULL, (strftime('%s','2026-10-07 10:41:00') - 978307200) * 1000000000, 0),
    (7, 'G7', 'hey are you around?', 1, 'iMessage', 0, (strftime('%s','2026-10-07 10:42:00') - 978307200) * 1000000000, 0, 0, 0, 0, NULL, 0, NULL, 0, 0),
    (8, 'G8', NULL, 1, 'iMessage', 0, (strftime('%s','2026-10-07 10:43:00') - 978307200) * 1000000000, 0, 0, 0, 0, NULL, 0,
        X'040B73747265616D747970656481E803840140848484124E5341747472696275746564537472696E67008484084E534F626A65637400858492848484084E53537472696E67019484012B2574686973206F6E65206F6E6C79206861732061747472696275746564426F647920F09F938E8684026949012092848484', 0, 0),
    (9, 'G9', NULL, 1, 'iMessage', 0, (strftime('%s','2026-10-07 10:44:00') - 978307200) * 1000000000, 0, 1, 0, 0, NULL, 0, NULL, 0, (strftime('%s','2026-10-07 10:45:00') - 978307200) * 1000000000),
    -- chat 2: SMS, one failed send
    (10, 'G10', 'ok sounds good', 3, 'SMS', 0, (strftime('%s','2026-10-07 09:15:00') - 978307200) * 1000000000, 0, 0, 0, 0, NULL, 0, NULL, 0, 0),
    (11, 'G11', 'see you then', 0, 'SMS', 22, (strftime('%s','2026-10-07 09:16:00') - 978307200) * 1000000000, 1, 1, 0, 0, NULL, 0, NULL, 0, 0),
    -- chat 3: unnamed group
    (12, 'G12', 'who''s bringing the speaker', 2, 'iMessage', 0, (strftime('%s','2026-10-07 08:00:00') - 978307200) * 1000000000, 0, 1, 0, 0, NULL, 0, NULL, 0, 0),
    (13, 'G13', 'not me', 5, 'iMessage', 0, (strftime('%s','2026-10-07 08:01:00') - 978307200) * 1000000000, 0, 1, 0, 0, NULL, 0, NULL, 0, 0),
    (14, 'G14', 'me', 0, 'iMessage', 0, (strftime('%s','2026-10-07 08:01:30') - 978307200) * 1000000000, 1, 1, 0, 0, NULL, 0, NULL, 0, 0),
    (15, 'G15', 'lol', 5, 'iMessage', 0, (strftime('%s','2026-10-07 08:02:00') - 978307200) * 1000000000, 0, 0, 0, 0, NULL, 0, NULL, 0, 0),
    -- system row (participant added): must be skipped
    (16, 'G16', NULL, 2, 'iMessage', 0, (strftime('%s','2026-10-07 08:02:30') - 978307200) * 1000000000, 0, 1, 0, 1, NULL, 0, NULL, 0, 0),
    -- tapback removal: must be skipped
    (17, 'G17', 'Removed a heart', 2, 'iMessage', 0, (strftime('%s','2026-10-07 08:03:00') - 978307200) * 1000000000, 0, 1, 0, 0, 'p:0/G13', 3000, NULL, 0, 0),
    -- chat 4: attachment only
    (18, 'G18', char(65532), 4, 'iMessage', 0, (strftime('%s','2026-10-05 19:00:00') - 978307200) * 1000000000, 0, 1, 1, 0, NULL, 0, NULL, 0, 0),
    -- chat 5: named group, older
    (19, 'G19', 'see ya', 2, 'iMessage', 0, (strftime('%s','2026-10-04 21:00:00') - 978307200) * 1000000000, 0, 1, 0, 0, NULL, 0, NULL, 0, 0);

INSERT INTO chat_message_join (chat_id, message_id, message_date)
    SELECT CASE
             WHEN ROWID <= 9 THEN 1
             WHEN ROWID <= 11 THEN 2
             WHEN ROWID <= 17 THEN 3
             WHEN ROWID = 18 THEN 4
             ELSE 5
           END, ROWID, date
    FROM message;

INSERT INTO attachment (ROWID, guid, filename, mime_type, transfer_name, total_bytes) VALUES
    (1, 'A1', '~/Library/Messages/Attachments/aa/00/A1/IMG_1234.heic', 'image/heic', 'IMG_1234.heic', 1234567);
INSERT INTO message_attachment_join VALUES (18, 1);

-- Receipts on sent messages (Apple-epoch ns): row 2 delivered and read, row 6 delivered only.
UPDATE message SET date_delivered = (strftime('%s','2026-10-07 10:33:05') - 978307200) * 1000000000,
                   date_read      = (strftime('%s','2026-10-07 10:34:00') - 978307200) * 1000000000 WHERE ROWID = 2;
UPDATE message SET date_delivered = (strftime('%s','2026-10-07 10:40:03') - 978307200) * 1000000000 WHERE ROWID = 6;
-- On an incoming row date_read is when you read it; it must not surface as a receipt.
UPDATE message SET date_read = (strftime('%s','2026-10-07 10:31:30') - 978307200) * 1000000000 WHERE ROWID = 1;

-- One message row attached to two chats (Messages does this now and then); it
-- must come back once from a merged history and once from `since`.
INSERT INTO message (ROWID, guid, text, handle_id, service, date, is_from_me, is_read)
    VALUES (23, 'G23', 'in two chats', 4, 'iMessage', (strftime('%s','2026-10-03 12:00:00') - 978307200) * 1000000000, 0, 1);
INSERT INTO chat_message_join VALUES (4, 23, (strftime('%s','2026-10-03 12:00:00') - 978307200) * 1000000000);
INSERT INTO chat_message_join VALUES (5, 23, (strftime('%s','2026-10-03 12:00:00') - 978307200) * 1000000000);

-- A non-picture attachment: never served as an image.
INSERT INTO attachment (ROWID, guid, filename, mime_type, transfer_name, total_bytes) VALUES
    (2, 'A2', '~/Library/Messages/Attachments/bb/01/A2/notes.pdf', 'application/pdf', 'notes.pdf', 2048);
INSERT INTO message_attachment_join VALUES (19, 2);
