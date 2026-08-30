# Third-party and local-only components

The Aura source repository contains Aura-owned code and integration code. It
does not redistribute installed commercial applications, plugin binaries,
voicebanks, rendered audio, or build products.

## OpenUtau

OpenUtau is maintained in the local review clone at `.openutau-review/` and is
not part of the Aura Git history or release bundle. The upstream repository is:

<https://github.com/stakira/OpenUtau>

The upstream project includes its own MIT license and copyright notice. If
OpenUtau source is redistributed, its license and notices must remain intact.
Aura only ships the OpenUtau bridge and import/export contract unless a
separate, explicit redistribution decision is made.

## Surge XT and Vital

Surge XT and Vital are external plugin installations discovered from the host
system. Their binaries are not included in this repository. Users must obtain
and install them from their respective upstream/distribution channels and
accept their licenses separately.

## Voicebanks and audio assets

OpenUtau voicebanks, WAV/AIFF files, presets, and rendered examples are local
user assets. They must not be committed to the Aura source repository unless
their redistribution license is documented explicitly.

## Generated artifacts

`build/`, `target/`, `dist/`, `packaging/`, and `build-tools/` are local build or
packaging outputs and are excluded from the source publication scope.

## Aura code

Aura-owned source is offered under the MIT terms in `LICENSE`, subject to
the separate license requirements of any third-party code that is copied into
the repository or linked into a combined distribution.
# Slint

The Aura GUI uses Slint. Slint packages declare
`GPL-3.0-only OR LicenseRef-Slint-Royalty-free-2.0 OR LicenseRef-Slint-Software-3.0`.
Aura's MIT license does not relicense Slint. Anyone distributing a combined GUI
binary must select and comply with an applicable Slint license.

# IBM Plex

The UI redistributes IBM Plex font files. Copyright © 2017 IBM Corp., with
Reserved Font Name "Plex". The fonts remain licensed under the SIL Open Font
License 1.1; see `aura-ui/resources/fonts/LICENSE-IBM-PLEX.txt`. Aura's MIT
license does not relicense the font files.
