-- Demo Contacts data for screenshots. Made-up people; load after the schema in
-- bridge/tests/fixtures/make_contacts.sql (see run.sh).

INSERT INTO ZABCDRECORD VALUES
    (1, 22, NULL, 'Maya', 'Chen', NULL),
    (2, 22, NULL, 'Jordan', 'Lee', NULL),
    (3, 22, NULL, 'Sam', 'Rivera', NULL),
    (4, 22, NULL, 'Dad', NULL, NULL),
    (5, 22, NULL, 'Priya', 'Patel', NULL);

INSERT INTO ZABCDPHONENUMBER VALUES
    (1, 1, '(555) 010-0001'),
    (2, 2, '(555) 010-0002'),
    (3, 3, '(555) 010-0004'),
    (4, 4, '(555) 010-0005'),
    (5, 5, '(555) 010-0008');

INSERT INTO ZABCDEMAILADDRESS VALUES
    (1, 2, 'jordan.lee@example.com');
