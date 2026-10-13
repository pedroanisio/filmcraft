# Scene documents and scene-render

FilmCraft imports **sr-core scene documents** (`.scene`) as rendered media. A scene clip behaves
like any other clip, and its frames and audio come from **scene-render**, a separate program that
FilmCraft starts as needed. The bridge is the `filmcraft-scenerender` crate (`crates/scenerender`,
layer L3).

scene-render is separate software, installed and licensed on its own. FilmCraft never links it,
bundles it or ships it: it only runs the executable the user points it to, the way an
operating-system codec is used when present. With no renderer installed, scene documents still import
and edit normally and show a placeholder.

## Installing and finding the renderer

FilmCraft needs **scene-render 0.5.0 or newer**. It looks for the executable in this order:

1. **Preferences ▸ Plugins ▸ scene-render**: an explicit path to the executable.
2. The `FILMCRAFT_SCENE_RENDER` environment variable: a path to the executable.
3. `scene-render` (`scene-render.exe` on Windows) in a directory on `PATH`.

A candidate that does not exist is skipped, and the search moves to the next source. The chosen
executable is checked with `scene-render --version` and `scene-render capabilities`, each with a
30-second limit. The result is cached per executable (its resolved path, size and modification time),
so replacing the binary triggers a new check.

`scene.renderer.status` reports what was found (`found`, `path`, `version`, `schema`, `reason`).
scene-render renders on the GPU through wgpu; a software Vulkan driver such as Mesa's lavapipe also
works. It needs the fonts its documents use to be installed.

The web build cannot start external programs, so scene documents always show the placeholder there.

## Importing

`File ▸ Import` (or `file.import`) of a `.scene` file creates **one media item**. FilmCraft reads
the item's size, frame rate and duration from the document's `<project>` header itself, so this works
without the renderer. The project stores the item as an ordinary file reference, and projects that
use scene documents open on machines without scene-render.

When the renderer is missing, too old or failing its checks, the import result carries a warning
saying why. The item then shows a **placeholder slate**, diagonal stripes at the scene's size, with
silent audio. This is distinct from the offline-media slate: the document is present, but nothing
can render it. The item is rendered for real once the renderer becomes available and the preference
is applied.

Assets the document references (images, video, audio, fonts) are resolved by scene-render relative
to the document's directory, as scene-render always does.

## Rendering and timing

- **Frames.** Frame *k* of a scene is its state at *k*·den/num seconds of the project frame rate.
  The frame count is the duration multiplied by the rate, rounded up (exact in `Tick`s). A request
  past the end shows the last frame.
- **Chunks.** Starting the renderer and its GPU is expensive, so frames are rendered in chunks
  (48 frames, aligned to multiples of 48). Only the frames of a chunk that are not already cached are
  requested. At most one scene-render process runs at a time across the application, and each process has a ten-minute limit.
  Cancelling playback or an export stops the running process.
- **Quality.** Playback and render previews use the preview quality from **Preferences ▸ Plugins**
  (`preview` by default, or `draft`). While scrubbing, FilmCraft asks for `draft`, which
  scene-render draws at half size; FilmCraft scales those frames back up to the scene's size.
  **Exporting always renders at `final` quality.**
- **Alpha.** Frames are 8-bit straight-alpha sRGB, so a scene can be used as an overlay on a
  higher track.
- **Audio.** On the first audio request, FilmCraft runs `scene-render encode` once to mix the
  scene's audio to a 48 kHz WAV, then plays from that file. Multichannel mixes are folded to
  stereo: centre to both sides at −3 dB, LFE dropped, surround pairs to their side at −3 dB. A
  scene with no audio, or a mixdown that fails, plays silence; the failure is logged once.
- **Failures.** A chunk the renderer cannot produce (an invalid document, a renderer crash, a wrong-sized
  frame) is remembered and shows as a media error instead of being retried on every frame. After
  4096 distinct failures the source stops trying altogether. Editing the document changes its cache key, so it is rendered again.

## The render cache

Rendered frames are kept in memory (up to 256 MiB per scene) and on disk in the media cache:

```
<media cache>/Scene Renders/<key>/<quality>/<frame:06>.png
<media cache>/Scene Renders/<key of final>/mix.wav
```

The key is a SHA-256 over the renderer version, the quality, the document's bytes and, for each
referenced asset, its resolved path, size and modification time. Editing the document or an asset, or
upgrading scene-render, therefore renders again rather than reusing stale frames. A chunk is rendered into a fresh staging directory. Each
frame is checked (a valid PNG at the expected size) and only then moved into place, so an
interrupted render never leaves a partial frame behind.

`scene.renderer.clearCache` deletes `Scene Renders` and returns the bytes freed. Clearing the media
cache in Preferences also removes it. Without a media cache directory (the web build), frames are
kept in memory only.

## Commands

| Command | Params | Result |
|---|---|---|
| `scene.renderer.status` | none | `{found, path, version, schema, reason}` for the current preference |
| `scene.renderer.clearCache` | none | `{bytes}` freed; an error when there is no media cache directory |

Full parameter details are in [control-protocol.md](control-protocol.md).

## Known limitations

- Ambisonic mixes are folded to stereo by the same channel rule as speaker layouts, without
  decoding them.
- If scene-render leaves a child process running that holds its output pipes, the run ends only at
  the process time limit.
- Staging directories left behind by a crash are reclaimed only when the cache is cleared.
- If moving a chunk's frames into the cache fails part-way, the frames moved before the failure stay
  cached. They are complete and valid, and the rest are rendered on the next request.
- Without a media cache directory, the audio mixdown lives in a temporary file until the item is
  closed.
- Scene documents are rendered as one flat clip. Mapping a scene's layers onto FilmCraft tracks
  (scene import as a sequence) is separate work, tracked with the scene interchange tasks.

## For developers

- The crate never panics on hostile input. Every number from the document header, the filesystem,
  the renderer's output and the environment is bounded.
- Tests use a fake renderer: a `sh` script, generated per test, that follows the scene-render
  command line (unix only).
- A real-renderer check is opt-in. Set `SR_RENDER_BIN=/path/to/scene-render` and run
  `cargo test -p filmcraft-scenerender source_real_renderer_oracle`. It renders a small scene,
  checks the frame size and content, and checks that a second open reads from the disk cache
  without rendering again.
