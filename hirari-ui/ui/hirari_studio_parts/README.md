# HIRARI Studio Slint Part Contract

The split files are concatenated in numeric order by `build.rs`. The
`app_window_tail` and `dialogs_and_tools_tail` files are continuation fragments
used to keep each source part under the 600-line limit. They are not independent
Slint modules; the order is an explicit build contract.
`editor_views_tail` continues `editor_views` in the same way.

## Dependency direction

`app_window` and `app_window_tail` own imports, exported action globals, model
types, and `AppWindow`.
`automation_view`, `editor_views`, and `editor_views_tail` contain primary workflow layout and view composition.
`dialogs_and_tools` and its tail contain telemetry/status layout and the closing
window structure.
Parts may consume `AppWindow` properties and action globals, but may not define
another root component or import experimental views.

## State ownership

- Core owns audio, project, routing, plugin, transport, and undo state.
- Rust UI modules own callbacks, model replacement, and telemetry scheduling.
- `AppWindow` owns only presentation state and transient selection/panel state.
- A project load/recovery must replace the track model and clear project-scoped
  presentation state before the next telemetry tick.

## Change rules

Keep each part at or below 600 lines. Add shared state to the root contract only
when Rust has a corresponding setter/getter and a project-load reset path.
`build.rs` rejects duplicate `AppWindow` declarations and experimental imports.
