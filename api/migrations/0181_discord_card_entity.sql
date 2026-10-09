-- Which item a show's channel card is currently showing.
--
-- A replacement file (an upgrade, a repack) edits the item's existing message in
-- place instead of announcing it again. The card carries a copy of that message,
-- so it is edited too — but only while it is still showing that item.
ALTER TABLE discord_show_threads ADD COLUMN card_entity_id UUID;
