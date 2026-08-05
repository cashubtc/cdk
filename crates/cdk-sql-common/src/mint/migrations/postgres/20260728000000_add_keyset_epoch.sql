CREATE TABLE keyset_epoch (
    id INTEGER PRIMARY KEY,
    epoch BIGINT NOT NULL
);
INSERT INTO keyset_epoch (id, epoch) VALUES (0, (SELECT COUNT(*) FROM keyset));
