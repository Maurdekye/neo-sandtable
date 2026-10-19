-- Approved SOURCE catalog, lead m5b7759e8ebc9. No execution or activation proof.
-- Same 35 SQL literals as review FC68873E. All values use bound parameters.
-- J01 configuration immediately after safe open, before schema access
PRAGMA main.cache_size=-128;
-- J02
PRAGMA mmap_size=0;
-- J03
PRAGMA cache_spill=ON;
-- J04
PRAGMA temp_store=FILE;
-- J05
PRAGMA main.journal_mode=DELETE;
-- J06
PRAGMA main.synchronous=FULL;
-- J07
PRAGMA foreign_keys=ON;
-- J08
PRAGMA trusted_schema=OFF;
-- J09 new file only; existing file must already have the expected page size
PRAGMA main.page_size=4096;
-- J10
PRAGMA main.page_size;
-- J11
PRAGMA main.cache_size;
-- J12
PRAGMA mmap_size;
-- J13
PRAGMA main.synchronous;
-- J14
PRAGMA foreign_keys;
-- J15
PRAGMA main.journal_mode;
-- J16 stream compile-option rows; no collect or replacement allocator
PRAGMA compile_options;
-- J17 stream exact schema; reject unexpected objects/definitions
SELECT type,name,tbl_name,sql FROM main.sqlite_schema;
-- J18
CREATE TABLE host(singleton INTEGER PRIMARY KEY CHECK(singleton=1),format INTEGER NOT NULL,generation INTEGER NOT NULL CHECK(generation>0),incarnation BLOB NOT NULL CHECK(length(incarnation)=16),phase INTEGER NOT NULL CHECK(phase IN(0,1,2)),freeze_id BLOB,expected_writers INTEGER,CHECK(freeze_id IS NULL OR length(freeze_id)=16));
-- J19
CREATE TABLE member(generation INTEGER NOT NULL,scope INTEGER NOT NULL,campaign TEXT NOT NULL,storage TEXT NOT NULL,rules_profile TEXT NOT NULL,content_hash TEXT NOT NULL,engine_version TEXT NOT NULL,PRIMARY KEY(generation,storage),UNIQUE(generation,campaign));
-- J20
CREATE TABLE writer(generation INTEGER NOT NULL,scope INTEGER NOT NULL,campaign TEXT NOT NULL,storage TEXT NOT NULL,rules_profile TEXT NOT NULL,content_hash TEXT NOT NULL,engine_version TEXT NOT NULL,revision INTEGER NOT NULL,terminal_state TEXT NOT NULL,writer_joined INTEGER NOT NULL CHECK(writer_joined=1),durable_ack INTEGER NOT NULL CHECK(durable_ack=1),PRIMARY KEY(generation,storage),UNIQUE(generation,campaign));
-- J21 read latest generation and all classification fields, without selecting an older CLEAN
SELECT format,generation,incarnation,phase,freeze_id,expected_writers FROM host WHERE singleton=1;
-- J22 first file insertion or checked advance of the current generation; zero changed rows refuses OPEN
INSERT INTO host(singleton,format,generation,incarnation,phase,freeze_id,expected_writers) VALUES(1,?1,?2,?3,0,NULL,NULL) ON CONFLICT(singleton) DO UPDATE SET format=excluded.format,generation=excluded.generation,incarnation=excluded.incarnation,phase=0,freeze_id=NULL,expected_writers=NULL WHERE host.generation=?4 AND host.format=?1;
-- J23 check a stored writer identity before a duplicate/refusal decision; no overwrite
SELECT scope,campaign,storage,rules_profile,content_hash,engine_version FROM member WHERE generation=?1 AND storage=?2;
-- J24 member insertion in the current OPEN generation only
INSERT INTO member(generation,scope,campaign,storage,rules_profile,content_hash,engine_version) SELECT ?1,?2,?3,?4,?5,?6,?7 WHERE EXISTS(SELECT 1 FROM host WHERE singleton=1 AND generation=?1 AND incarnation=?8 AND phase=0);
-- J25 stream owned members for exact set correspondence; indexed by generation/storage
SELECT scope,campaign,storage,rules_profile,content_hash,engine_version FROM member WHERE generation=?1 ORDER BY storage;
-- J26 freeze once after all factory ingress/attempts settle; no renewal of a shutdown deadline
UPDATE host SET phase=1,freeze_id=?1,expected_writers=?2 WHERE singleton=1 AND generation=?3 AND incarnation=?4 AND phase=0;
-- J27 frozen ALL-writer evidence, one bounded row at a time, after actual retained writer outcomes
INSERT INTO writer(generation,scope,campaign,storage,rules_profile,content_hash,engine_version,revision,terminal_state,writer_joined,durable_ack) SELECT ?1,?2,?3,?4,?5,?6,?7,?8,?9,1,1 WHERE EXISTS(SELECT 1 FROM host WHERE singleton=1 AND generation=?1 AND incarnation=?10 AND phase=1 AND freeze_id=?11);
-- J28 stream exact final writer rows for current-generation recovery/evidence validation
SELECT scope,campaign,storage,rules_profile,content_hash,engine_version,revision,terminal_state,writer_joined,durable_ack FROM writer WHERE generation=?1 ORDER BY storage;
-- J29 independent indexed cardinality for the frozen set; never substitutes for exact owner correspondence
SELECT count(*) FROM writer WHERE generation=?1;
-- J30 verify every prior owned member matches a recorded writer, including scope and all stored pins
SELECT m.storage FROM member AS m WHERE m.generation=?1 AND NOT EXISTS(SELECT 1 FROM writer AS w WHERE w.generation=m.generation AND w.storage=m.storage AND w.scope=m.scope AND w.campaign=m.campaign AND w.rules_profile=m.rules_profile AND w.content_hash=m.content_hash AND w.engine_version=m.engine_version) LIMIT 1;
-- J31 terminal CAS only after exact retained outcome/census checks; durable commit is the LAST write
UPDATE host SET phase=2 WHERE singleton=1 AND generation=?1 AND incarnation=?2 AND phase=1 AND freeze_id=?3 AND expected_writers=?4 AND (SELECT count(*) FROM writer WHERE generation=?1)=?4 AND NOT EXISTS(SELECT 1 FROM member AS m WHERE m.generation=?1 AND NOT EXISTS(SELECT 1 FROM writer AS w WHERE w.generation=m.generation AND w.storage=m.storage AND w.scope=m.scope AND w.campaign=m.campaign AND w.rules_profile=m.rules_profile AND w.content_hash=m.content_hash AND w.engine_version=m.engine_version));
-- J32 no dynamic savepoint names, scripts, ATTACH, VACUUM, migrations or arbitrary SQL
BEGIN IMMEDIATE;
-- J33 retain commit result before any clean observation
COMMIT;
-- J34 retain a separate rollback error if rollback is attempted after primary failure
ROLLBACK;
-- J35 validate actual row-bearing schema columns without loading user values or collecting rows
SELECT cid,name,type,"notnull",dflt_value,pk FROM pragma_table_info(?1);
