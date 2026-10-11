-- Demo Messages data for screenshots. People, numbers and text are made up.
-- Load after the schema from bridge/tests/fixtures/make_fixture.sql (see run.sh).
-- Times are relative to now; T(n) below means "n minutes ago" in Apple-epoch ns:
--   (strftime('%s','now','-n minutes') - 978307200) * 1000000000

INSERT INTO handle (ROWID, id, service) VALUES
    (1, '+15550100001', 'iMessage'),           -- Maya
    (2, '+15550100002', 'iMessage'),           -- Jordan (phone)
    (3, 'jordan.lee@example.com', 'iMessage'), -- Jordan (email): merged via Contacts
    (4, '+15550100004', 'iMessage'),           -- Sam
    (5, '+15550100005', 'iMessage'),           -- Dad
    (6, '+15550100777', 'SMS'),                -- pharmacy, not in Contacts
    (7, '+15550100008', 'iMessage');           -- Priya

INSERT INTO chat (ROWID, guid, style, chat_identifier, service_name, display_name) VALUES
    (1, 'iMessage;-;+15550100001', 45, '+15550100001', 'iMessage', ''),
    (2, 'iMessage;+;chat000000000000000101', 43, 'chat000000000000000101', 'iMessage', 'Hiking Club'),
    (3, 'iMessage;-;+15550100002', 45, '+15550100002', 'iMessage', ''),
    (4, 'iMessage;-;jordan.lee@example.com', 45, 'jordan.lee@example.com', 'iMessage', ''),
    (5, 'iMessage;-;+15550100005', 45, '+15550100005', 'iMessage', ''),
    (6, 'SMS;-;+15550100777', 45, '+15550100777', 'SMS', ''),
    (7, 'iMessage;-;+15550100008', 45, '+15550100008', 'iMessage', '');

INSERT INTO chat_handle_join VALUES (1,1), (2,1), (2,2), (2,4), (3,2), (4,3), (5,5), (6,6), (7,7);

-- (rowid, chat, handle, from_me, read, minutes ago, text)
CREATE TEMP TABLE m (rowid INTEGER, chat INTEGER, handle INTEGER, me INTEGER, rd INTEGER, ago INTEGER, text TEXT);
INSERT INTO m VALUES
    -- Maya
    (1,  1, 1, 0, 1, 52,   'are we still on for dinner tonight?'),
    (2,  1, 0, 1, 1, 50,   'yes! 7:30 at Lucia''s?'),
    (4,  1, 1, 0, 1, 48,   'perfect. I''ll book a table for 4'),
    (5,  1, 1, 0, 1, 47,   'Jordan and Sam are coming too'),
    (6,  1, 0, 1, 1, 30,   'great, I''ll pick up Sam on the way 🚗'),
    (7,  1, 1, 0, 1, 12,   'they only had 7:45, hope that''s ok'),
    (8,  1, 0, 1, 1, 11,   'works for me 👍'),
    (9,  1, 1, 0, 1, 2,    'see you there!'),
    -- Hiking Club
    (20, 2, 4, 0, 1, 190,  'trail report says the upper loop is open again'),
    (21, 2, 2, 0, 1, 170,  'saturday then? I can drive'),
    (22, 2, 0, 1, 1, 165,  'I''m in. bringing snacks'),
    (23, 2, 1, 0, 0, 20,   'forecast looks clear all weekend'),
    (24, 2, 4, 0, 0, 5,    'meet at the trailhead at 8?'),
    -- Jordan, by email (older) then by phone
    (30, 4, 3, 0, 1, 2900, 'sent you the photos from the dive'),
    (31, 3, 0, 1, 1, 1500, 'thanks for the photos! the reef one is amazing'),
    (32, 3, 2, 0, 1, 1490, 'anytime 🙌'),
    -- Dad
    (40, 5, 5, 0, 1, 1380, 'look how the tomatoes are doing'),
    (42, 5, 0, 1, 1, 1370, 'wow, that''s a lot of tomatoes 🍅'),
    -- Pharmacy (SMS)
    (50, 6, 6, 0, 0, 180,  'Your prescription is ready for pickup at Main St. Reply STOP to opt out.'),
    -- Priya
    (60, 7, 7, 0, 1, 4400, 'happy birthday!! 🎉'),
    (61, 7, 0, 1, 1, 4380, 'thank you!! see you next week');

INSERT INTO message (ROWID, guid, text, handle_id, service, date, is_from_me, is_read)
    SELECT rowid, 'D' || rowid, text, handle, CASE chat WHEN 6 THEN 'SMS' ELSE 'iMessage' END,
           (strftime('%s','now','-' || ago || ' minutes') - 978307200) * 1000000000, me, rd
    FROM m;

-- Maya loved "yes! 7:30 at Lucia's?"
INSERT INTO message (ROWID, guid, text, handle_id, service, date, is_from_me, is_read,
                     associated_message_guid, associated_message_type)
    VALUES (3, 'D3', 'Loved “yes! 7:30 at Lucia''s?”', 1, 'iMessage',
            (strftime('%s','now','-49 minutes') - 978307200) * 1000000000, 0, 1, 'p:0/D2', 2000);

-- Dad's photo
INSERT INTO message (ROWID, guid, text, handle_id, service, date, is_from_me, is_read, cache_has_attachments)
    VALUES (41, 'D41', char(65532), 5, 'iMessage',
            (strftime('%s','now','-1379 minutes') - 978307200) * 1000000000, 0, 1, 1);
INSERT INTO attachment (ROWID, guid, filename, mime_type, transfer_name, total_bytes)
    VALUES (1, 'A1', '~/Library/Messages/Attachments/00/A1/IMG_2041.heic', 'image/heic', 'IMG_2041.heic', 2345678);
INSERT INTO message_attachment_join VALUES (41, 1);

INSERT INTO chat_message_join (chat_id, message_id, message_date)
    SELECT COALESCE((SELECT chat FROM m WHERE m.rowid = message.ROWID),
                    CASE message.ROWID WHEN 3 THEN 1 WHEN 41 THEN 5 END),
           ROWID, date
    FROM message;

-- Receipts: everything you sent was delivered; most were read, the latest to Maya not yet.
UPDATE message SET date_delivered = date + 4000000000 WHERE is_from_me = 1;
UPDATE message SET date_read = date + 60000000000 WHERE is_from_me = 1 AND ROWID <> 8;
