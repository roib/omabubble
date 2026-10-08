-- Fake Contacts database (AddressBook-v22.abcddb). Made-up people and numbers.
-- Only the columns the bridge reads; the real tables have many more.

CREATE TABLE ZABCDRECORD (
    Z_PK INTEGER PRIMARY KEY,
    Z_ENT INTEGER,
    ZLINKID VARCHAR,
    ZFIRSTNAME VARCHAR,
    ZLASTNAME VARCHAR,
    ZORGANIZATION VARCHAR
);
CREATE TABLE ZABCDPHONENUMBER (
    Z_PK INTEGER PRIMARY KEY,
    ZOWNER INTEGER,
    ZFULLNUMBER VARCHAR
);
CREATE TABLE ZABCDEMAILADDRESS (
    Z_PK INTEGER PRIMARY KEY,
    ZOWNER INTEGER,
    ZADDRESS VARCHAR
);

INSERT INTO ZABCDRECORD VALUES
    (1, 22, NULL, 'Alex', 'Example', NULL),          -- phone + email on one card
    (2, 22, 'LINK-A', 'Pat', NULL, NULL),            -- two cards linked by Contacts
    (3, 22, 'LINK-A', 'Pat', 'Linked', NULL),
    (4, 22, NULL, NULL, NULL, 'Acme Corp'),          -- organization only
    (5, 22, NULL, 'Sam', NULL, NULL),                -- international number
    (6, 22, NULL, 'Nobody', 'Here', NULL),           -- no phone or email: dropped
    (7, 22, NULL, 'Group', NULL, NULL);              -- Z_ENT differs in real data; harmless

INSERT INTO ZABCDPHONENUMBER VALUES
    (1, 1, '(555) 000-0001'),
    (2, 2, '+1 555-000-0002'),
    (3, 3, '555 000 0004'),
    (4, 4, '+15550000003'),
    (5, 5, '+44 20 7946 0958'),
    (6, 7, '123');                                    -- too short to ever match

INSERT INTO ZABCDEMAILADDRESS VALUES
    (1, 1, 'Friend@Example.com'),
    (2, 4, 'billing@acme.example');
