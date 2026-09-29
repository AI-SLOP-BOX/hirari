# Third-party and local-only components

The Hirari source repository contains Hirari-owned code and integration code. It
does not redistribute installed commercial applications, plugin binaries,
voicebanks, rendered audio, or build products.

## OpenUtau

OpenUtau is maintained in the local review clone at `.openutau-review/` and is
not part of the Hirari Git history or release bundle. The upstream repository is:

<https://github.com/stakira/OpenUtau>

The upstream project includes its own MIT license and copyright notice. If
OpenUtau source is redistributed, its license and notices must remain intact.
Hirari only ships the OpenUtau bridge and import/export contract unless a
separate, explicit redistribution decision is made.

## Surge XT and Vital

Surge XT and Vital are external plugin installations discovered from the host
system. Their binaries are not included in this repository. Users must obtain
and install them from their respective upstream/distribution channels and
accept their licenses separately.

## Signalsmith Stretch and Signalsmith Linear

Hirari vendors the headers used for spectral time stretching and formant
processing from Signalsmith Stretch and its Signalsmith Linear dependency:

- Signalsmith Stretch: <https://github.com/Signalsmith-Audio/signalsmith-stretch>
  revision `57b93f4e9206a089a45387eaa39bdc9f310d3308`
- Signalsmith Linear: <https://github.com/Signalsmith-Audio/linear>
  revision `547f4a6c55b4243191a9180f39849d67cc66aa0d`

Both are distributed under the MIT License. Their original license texts are
retained in `src/external/signalsmith-stretch/LICENSE.txt` and
`src/external/signalsmith-linear/LICENSE.txt`.
Hirari's local change to Signalsmith Stretch snapshots transpose and formant
controls for each spectrum, so split real-time computation cannot observe a
mid-spectrum parameter change.

## Symphonia

The production Preview Audio Runtime links Symphonia 0.5.5 and its enabled
format/codec crates for PCM/Float WAV, AIFF, FLAC, MP3, Ogg/Vorbis, AAC, and
ISO MP4 audio. Symphonia and these decoder crates are licensed under MPL-2.0;
the license text and source are available from
<https://github.com/pdeljanov/Symphonia>. The selected codec/format features
are recorded in `hirari-core-bridge/Cargo.toml`.

## Voicebanks and audio assets

OpenUtau voicebanks, WAV/AIFF files, presets, and rendered examples are local
user assets. They must not be committed to the Hirari source repository unless
their redistribution license is documented explicitly.

## Generated artifacts

`build/`, `target/`, `dist/`, `packaging/`, and `build-tools/` are local build or
packaging outputs and are excluded from the source publication scope.

## Hirari code

Hirari-owned source is offered under the MIT terms in `LICENSE`, subject to
the separate license requirements of any third-party code that is copied into
the repository or linked into a combined distribution.
# Slint

The Hirari GUI uses Slint. Slint packages declare
`GPL-3.0-only OR LicenseRef-Slint-Royalty-free-2.0 OR LicenseRef-Slint-Software-3.0`.
Hirari's MIT license does not relicense Slint. Anyone distributing a combined GUI
binary must select and comply with an applicable Slint license.

# IBM Plex

The UI redistributes IBM Plex font files. Copyright © 2017 IBM Corp., with
Reserved Font Name "Plex". The fonts remain licensed under the SIL Open Font
License 1.1; see `hirari-ui/resources/fonts/LICENSE-IBM-PLEX.txt`. Hirari's MIT
license does not relicense the font files.
