ALTER TABLE reading_states
    ADD COLUMN client_payload JSONB;

COMMENT ON COLUMN reading_states.client_payload IS
    'Raw Kobo-shape JSON body last PUT by this device, or NULL for rows '
    'owned by non-Kobo writers (web reader, Hardcover inbound sync). '
    'Served byte-identical on GET when this device is the winning state '
    'or when no other device is meaningfully further ahead, so Nickel '
    'does not spuriously prompt "continue from another device?".';
