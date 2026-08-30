# Aura publication scope

## Publish

- Aura-owned Rust, C++, Slint, scripts, tests, and documentation.
- The OpenUtau bridge, project contract, and import/render integration.
- `LICENSE` and `THIRD_PARTY_NOTICES.md`.

## Do not publish by default

- `.openutau-review/` (local upstream clone used for review only).
- `build/`, `target/`, `dist/`, `packaging/`, and `build-tools/`.
- `/Applications/OpenUtau.app`, Vital, Surge XT, or any other installed app/plugin.
- Voicebanks, presets, project files, rendered audio, and private session data.
- Logs, local configuration, journals, and generated screenshots.

## Licensing rule

Adding an MIT license to Aura-owned files does not relicense third-party code.
Every copied dependency must retain its original copyright and license, and
every combined distribution must be reviewed separately from this source
publication.
