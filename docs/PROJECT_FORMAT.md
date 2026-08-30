# Aura project persistence contract

Aura has one user-facing project authority: `aura-core-bridge`'s
`ProjectDocument`. Rust persistence owns the schema version, generation, asset
manifest, plugin state envelope, and atomic publication of `.aura` files.

The native C++ serializer is retained only as a compatibility snapshot for
the native engine and hydrate rollback. It is not a second user-facing project
format and must never be used to publish a project file directly.

## Publication rules

- Writes use a unique sibling staging file, flush the file, atomically rename,
  and sync the parent directory where the platform supports it.
- Autosave, manual save, recovery, and recording publication use separate
  operation locks and monotonically increasing generations.
- A stale generation may not publish a render, waveform, asset, or project
  snapshot.
- Native hydrate is transactional: the current native snapshot is restored if
  any track, region, plugin, or audio configuration step fails.
- Plugin state is carried in the Rust state envelope with version, payload
  size, checksum, and completion marker. Native snapshots may be discarded
  after a successful hydrate.

## Identity rules

Track and region IDs are stable within the project document. On native reload,
allocators are reseeded above the highest restored track ID and region IDs are
allocated from a fresh document generation, preventing an edit after reload
from reusing an existing identity.

Commands must include the project and audio-configuration generations they
were based on when issued by an external client. The bridge rejects stale
commands before mutation.
