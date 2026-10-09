# Send to Kobbler (Calibre Plugin)

This is the MVP Calibre UI plugin that uploads metadata, cover, and KEPUB files to Kobbler.

## Install

1) Build the plugin zip from the repo root:
```
./scripts/build_plugin_zip.sh
```

Note: The zip filename must be `send_to_kobbler.zip` so Calibre loads it as `calibre_plugins.send_to_kobbler`. The
empty `plugin-import-name-send_to_kobbler.txt` file is required for multi-file plugins.

2) In Calibre:
- Preferences -> Plugins -> Load plugin from file

## Configure

- Kobbler Base URL: `http://localhost:6766`
- Ingest Token: from `scripts/bootstrap.py`

## Usage

Select books -> "Send to Kobbler"

The plugin automatically converts EPUB to KEPUB using Calibre's conversion pipeline if needed.

## Notes
- You can choose whether to store the KEPUB in the Calibre library.
- The plugin always creates/updates the book metadata and file in Kobbler.
