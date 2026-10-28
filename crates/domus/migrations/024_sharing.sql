CREATE TABLE share_bindings (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    location_id INTEGER NOT NULL REFERENCES locations(id),
    relative_path TEXT NOT NULL,
    native_body TEXT NOT NULL,
    UNIQUE(location_id,relative_path)
) STRICT;
CREATE TABLE share_visibility (
    id INTEGER PRIMARY KEY CHECK(id=1),
    generation INTEGER NOT NULL
) STRICT;
INSERT INTO share_visibility VALUES(1,0);
CREATE TABLE share_catalogs (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    visibility INTEGER NOT NULL,
    complete INTEGER NOT NULL DEFAULT 0 CHECK(complete IN (0,1)),
    last_error TEXT
) STRICT;
CREATE TABLE share_catalog_paths (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    catalog_id INTEGER NOT NULL REFERENCES share_catalogs(id) ON DELETE CASCADE,
    binding_id INTEGER NOT NULL REFERENCES share_bindings(id),
    relative_path TEXT NOT NULL,
    wire_path TEXT NOT NULL,
    wire_key TEXT NOT NULL,
    parent_wire TEXT NOT NULL,
    kind TEXT NOT NULL CHECK(kind IN ('directory','file')),
    scanned INTEGER NOT NULL DEFAULT 0,
    bytes INTEGER NOT NULL DEFAULT 0,
    evidence TEXT,
    directory_body TEXT,
    UNIQUE(catalog_id,wire_key)
) STRICT;
CREATE INDEX share_catalog_frontier ON share_catalog_paths(catalog_id,kind,scanned,id);
CREATE INDEX share_catalog_page ON share_catalog_paths(catalog_id,kind,id);
CREATE INDEX share_catalog_folders ON share_catalog_paths(catalog_id,kind,wire_path);
CREATE INDEX share_catalog_children ON share_catalog_paths(catalog_id,kind,parent_wire,id);
CREATE TRIGGER sharing_publication_intent AFTER INSERT ON publication_destinations
BEGIN UPDATE share_visibility SET generation=generation+1 WHERE id=1; END;
CREATE TRIGGER sharing_publication_receipt AFTER UPDATE OF verified_at,last_error,album_identity ON publication_destinations
BEGIN UPDATE share_visibility SET generation=generation+1 WHERE id=1; END;
CREATE TRIGGER sharing_binding_insert AFTER INSERT ON share_bindings
BEGIN UPDATE share_visibility SET generation=generation+1 WHERE id=1; END;
CREATE TRIGGER sharing_binding_immutable BEFORE UPDATE ON share_bindings
BEGIN SELECT RAISE(ABORT,'share bindings are immutable; select a new folder explicitly'); END;
CREATE TRIGGER sharing_scan_changed AFTER UPDATE OF observed_at,coverage,last_error ON locations
WHEN NEW.observed_at IS NOT OLD.observed_at OR NEW.coverage IS NOT OLD.coverage OR NEW.last_error IS NOT OLD.last_error
BEGIN UPDATE share_visibility SET generation=generation+1 WHERE id=1; END;
