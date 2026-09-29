# Hirari and Moufu integration

Moufu remains a separate Integration Hub process. Hirari contains only an
optional client adapter and a small connection-management page; it does not
start or embed the Hub. Hub behavior and project-contract compatibility belong
in the separate Moufu repository. Hirari's pinned Moufu protocol revision does
not need to change for Hub-side contract validation: the entity metadata and
`OutOfSync` link status already exist in the pinned wire protocol. The new
reconnect validation becomes available when users run a Moufu Hub build that
contains it; Hirari remains a separate optional client.

Moufu's built-in `HirariProjectV2Extension` also rejects a published snapshot
whose inline JSON envelope does not match `hirari-project-layout-v2` version 2,
omits required project arrays, or has an invalid sample rate or tempo. The Hub
performs this check before coalescing or dispatch, so linked consumers never
receive a payload that fails the declared top-level contract.

Hirari can publish its current project layout to the Moufu Integration Hub.
Open **Window → Moufu** and enter the Hub's TCP address (for example
`127.0.0.1:9478`) to connect. The address is saved in the Hirari user
configuration directory and can be changed or disconnected while Hirari is
running. `HIRARI_MOUFU_ADDR` remains available as a launch-time override.

When enabled, Hirari registers one entity named **Current Hirari Project**:

- Entity ID: `hirari://workspace/current-project`
- Data type: `application/vnd.moufu.project+json` (Moufu's shared project type)
- Contract metadata: `hirari-project-layout-v2`
- Payload: a versioned object containing the native track layout, MIDI notes,
  sample rate, tempo and meter maps, and markers. UI-owned one-based
  `recording_input_channels` are merged into Audio/Vocal tracks on the adapter
  worker.

The **Moufu** page in Hirari's Window menu configures the Hub connection, lists
project entities and links, and lets the user explicitly create or remove a
link. Changing or removing the address closes the old TCP connection; the
latest pending project snapshot remains available if the user reconnects.
Link creation, removal, and catalog refresh requests are sent on the live
connection. Requests and received events carry a local connection generation,
so pending work from an old address is discarded when switching Hubs. Hirari
advertises the shared Moufu project media type and lists every other entity
using that type, including projects from apps that do not declare Hirari's
optional Hirari layout metadata. The page only establishes sharing links; it
never opens or replaces the current project with remote data.
The page requests the Hub's `ListEntities` and `ListLinks` snapshots, so an older
Hub that does not implement those requests can still connect and publish, but
cannot populate the management lists.

While an outbound project link is active, the UI checks a monotonic native
layout revision at 4 Hz. The revision advances at track/region/plugin/route
mutation points, and realtime automation recording marks a dirty flag that the
control-side poll folds into the revision. Rust-owned Aux identity changes use
the same native revision. With no active outbound link or while Moufu is
disconnected, the revision check is skipped. Track input assignments are UI-owned,
so changing an input in MixConsole publishes a fresh transient snapshot directly
instead of waiting for a native project revision. The full JSON snapshot is built
only when the revision changes, then published as a transient update. This
keeps the timer check O(1); Core still serializes the changed project layout
synchronously on the UI thread. The revision is an edit notification,
not a content hash: it does not notice external filesystem changes such as a
media file disappearing until another project edit causes a snapshot. When
publishing, the UI passes the owned JSON string and the small UI-owned input
channel list to the connector; JSON parsing, input-channel merging, and removal
of private fields happen on the Moufu worker, away from the UI thread. This
avoids reparsing and reserializing the complete layout in the UI just to add
input assignments. The connector
sends snapshots only while Hirari has an active outbound link; it keeps the
latest snapshot pending so a newly active or restored link receives the current
state immediately. When another outbound link becomes active without a project
edit, Hirari advances the entity version and republishes the stored snapshot;
Moufu rejects duplicate or stale entity versions, so reusing the previous
version would leave that peer without the current project. Project open and save
publish committed snapshots. Hirari parses and checks the layout, MIDI notes,
markers, sample rate, tempo, and packed tempo/meter maps before queuing a
snapshot; a malformed component cancels the whole publication rather than
being replaced by an empty array and sent as if it were valid. The adapter
discards its cached snapshot on this failure so a later link cannot receive an
earlier project's payload under the current session. Before publishing, the adapter removes
path-bearing fields and plug-in state fields recursively, normalizing key
separators so snake_case and camelCase forms are covered. Raw `state_blob` and
GUI state fields are also excluded. Track and region names, arrangement
details, MIDI authoring data, tempo, markers, and mixer values remain part of the shared snapshot. Moufu coalesces
transient changes and sends them only to the
owners of linked target entities. The integration advertises live links and
link restoration. Hirari receives link, state, and update notifications. The
Moufu page shows a read-only summary of the latest linked project snapshot
(version, track names, region counts, MIDI-note count, tempo, and marker count); shared-memory payloads are identified
without dereferencing them. The UI tracks versions per source, drops duplicate or
stale snapshots, and clears that version when the inbound link disappears. The
adapter bounds its event queue and coalesces queued project states by source,
keeping the newest committed state from being crowded out by transient edits.
Remote project edits are never applied to the open project.

When Hirari is the target of an active source-to-target link, it requests the
source's current state once the link becomes active. The result is delivered
through the same inbound state event as later updates; Hirari does not merge or
load remote state into the open project automatically. Hirari accepts inbound
state and update payloads only for sources with an active link to its registered
project entity, even if the Hub sends unrelated notifications. A request
rejection is reported to the UI without tearing down an otherwise healthy
connection.

The connector reconnects when Moufu is unavailable or a connection is lost,
resolving the configured host again on each retry and trying every returned
IPv4/IPv6 socket address before backing off. It uses a heartbeat to detect
half-open TCP connections, caps inbound messages at 8 MiB, and applies
backpressure instead of queueing unlimited server messages. Dropping the last
application handle signals the detached connection worker to stop; temporary
clones used by UI callbacks keep the connection alive until the UI releases
them. It also understands
Moufu's link-destruction notification even when built against the older pinned
protocol crate, so removing a link immediately revokes that source from the
inbound allowlist without forcing a disconnect/reconnect cycle.
Hirari does not send audio media through this integration. Project snapshots
are capped at 8 MiB, matching the inbound frame limit; an oversized snapshot
is skipped with a visible `PROJECT_SNAPSHOT_TOO_LARGE` notice while the link
stays connected. The address is opt-in; only configure it for a Moufu server
you trust. The saved setting is a plain TCP address, not a credential. Project
and track names and MIDI authoring data can contain text the user entered, so
use the link only with a trusted server.

Moufu's wire messages and entity types are defined by the pinned
`moufu-protocol` crate. The Hub tests check that Hirari project changes reach
linked targets only, the SDK tests cover handshake and disconnect behavior, and
the Hirari adapter test covers snapshot publishing and inbound update delivery.
