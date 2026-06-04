-- Runs once on first Postgres init (mounted into /docker-entrypoint-initdb.d).
-- The services also create their own database if missing; this just makes the
-- local stack work even if that path changes.
CREATE DATABASE synchronizer;
CREATE DATABASE relayer;
