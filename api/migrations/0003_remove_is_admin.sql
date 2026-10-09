-- All users are admins for now, drop vestigial field
ALTER TABLE users DROP COLUMN is_admin;
