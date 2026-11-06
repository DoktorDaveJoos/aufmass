# Aufmaß

Reads the DWG/DXF plans an architect sends and lists every wall, slab and roof
with its quantities, as preparation for a cadwork model and an offer.

```bash
aufmass path/to/Planeingang/            # folder: all .dwg/.dxf inside, recursively
aufmass eg.dwg dg.dwg -o auswertung/    # single files, custom output folder
aufmass plans/ -c mein-buero.toml       # custom config
```

Output (default: a `aufmass` folder next to the input):

| File | Content |
|---|---|
| `mengen.xlsx` | Totals per element type and storey, every element, every opening, skipped items |
| `building.json` | Neutral building model in world coordinates (meters): outlines, normals, thicknesses, openings. Input for the cadwork import. |
| `vorschau-<storey>.svg` | Plan view of the detected elements over the original linework, for checking |
| `mengen.x83` | GAEB DA XML 3.2 bill of quantities (phase 83, no prices) for PAPA or any other trade software: one Titel per storey, one position per element name |

## How it works

Many architect programs (here: Vectorworks via the ODA exporter) put the 3D model
into the DWG as `3DSOLID` entities. The converter uses those instead of guessing
from 2D lines:

1. `acadrust` reads DWG and DXF natively, including the ACIS data of the solids.
   DXF 2000–2018, ASCII and binary, give the same result as the DWG (checked on
   the sample set). Files that went through a converter (e.g. LibreDWG) often lose
   the 3D data; the tool says so instead of returning an empty list.
2. Each solid is rebuilt as planar faces in world coordinates (`src/acis.rs`).
3. The layer decides the element type (`Wand` = wall, `Decke` = slab,
   `Dachhaut` = roof). Per type (`src/classify.rs`):
   - **Wall**: thickness along the vertical-face normal with the smallest extent;
     length, height and area from the largest face. Openings are holes in that
     face (windows) or cut-outs from its bottom or ends (doors, corner glazing).
     A wall is exterior when rays from one side hit no other wall.
   - **Slab**: largest horizontal face. Flat, large solids only; furniture and
     stairs drawn on the same layer are skipped.
   - **Roof**: largest sloped face, with pitch.

   Volumes are those of the solids themselves, so a knee wall cut by the roof
   or a roof plate with plumb-cut eaves is not counted as a full box. Subtotals
   per element type show count and volume only: the layers of one build-up are
   separate rows, so adding their areas would count the same wall twice.
4. Solids standing on hatches of an "existing" color (grey in this office's
   plans), and everything touching them, are skipped as existing buildings.
   Solids repeated in several files (site plan) are counted once.

Every multi-layer build-up stays split as the architect modeled it, e.g.
36.5 cm frame wall + 6 cm cladding. Names per thickness come from the config.

## Config

`aufmass.toml` (in the working directory, or `-c`) sets layer names,
the existing-building color, filters and names. Without one, the built-in
copy of this repository's file is used. One config per architect office is
the expected setup.

## PAPA and cadwork

The desktop app has two hand-over buttons. They find the programs on their own
(`src/programs.rs`): the path in the config if set, else the program picked
before, else the installed programs (Windows uninstall entries) and the usual
folders (`C:\cadwork\v*\ci.exe`, the newest version; a folder named PAPA under
Program Files). When nothing is found, the app asks for the program file once
and keeps it in `%APPDATA%\aufmass\programme.toml`. A path in the config always
wins, e.g. to correct a wrong pick:

```toml
[papa]
path = 'C:\Programme\PAPA\PAPA.exe'

[cadwork]
path = 'C:\cadwork\v30\ci.exe'
```

- **In PAPA öffnen** starts PAPA and copies the path of `mengen.x83`. PAPA cannot
  be handed a file on start, so the user opens the project, chooses GAEB import
  and pastes the path. Positions are in m² (net area); a `[[naming]]` entry with
  `unit = "m3"` prices that build-up by volume instead.
- **In cadwork öffnen** installs the plugin `cadwork/Aufmass_Import` into every
  cadwork user profile (`%APPDATA%\cadwork\userprofil_*\3d\API.x64`) and starts
  cadwork. In cadwork the user runs the plugin once: it loads the latest
  `building.json` (recorded in `%APPDATA%\aufmass\letzte-auswertung.txt`) and
  creates every element as a polygon panel with window holes cut out, named after
  the element, grouped by storey. Without a cadwork profile the app saves the
  plugin next to the results for manual copying.

## Limits

- Needs 3D solids. Pure 2D drawings are rejected for now.
- The storey is the file name (`eg.dwg` -> `EG`). Solids repeated in several
  files are counted for the first file whose name names a storey (EG, 1.OG,
  Dachgeschoss, …), so a site plan sorted before the floor plans does not take
  them over.
- Existing buildings are found through connected solids: an extension that
  touches the existing building may be skipped with it. Not yet tested on
  such a plan set.
- Door/window type is guessed from sill height; opening heights and widths are
  measured on the face, not read from window objects.
- Only planar faces; curved edges are approximated by their vertices.

## Desktop app

`app/` is a GPUI desktop app (`aufmass-app`) on top of the same library. It
opens on a start window laid out as Thermkern's (energieausweis-express,
`desktop/`): the projects opened lately on the left, newest first and grouped
by day, and "Neues Projekt" and "Projekt öffnen …" on the right. Arrow keys and
Enter open a project, ⌘⌫ / Entf takes it off the list; ⌘N, ⌘O, ⌘W and ⇧⌘1
(Strg on Windows) make, open and close projects and bring the start window
back, and on macOS the menu Ablage › Zuletzt geöffnet lists them too.

A project is a small JSON file, `<Name>.aufmass`, suggested in Dokumente/Aufmaß:
its format, when and by which version it was made, and the plan folders or
files evaluated last (`app/src/project.rs`). Each project opens in a window of
its own and evaluates its plans again, so it shows their current state; closing
the last one brings the start window back. Plans dropped onto the start window
become a new project; in a project window, plans dropped onto it or chosen with
"Ordner wählen …" / "Dateien wählen …" replace its plans. The recent list is
`recent.json` beside `programme.toml` in `%APPDATA%\aufmass` (elsewhere
`~/.config/aufmass`).

A project window writes the same output folder as the command line and shows the quantities with
subtotals per element type (for the whole building or one storey), the plan
previews per storey ("Plan EG", …) and the model ("Modell") in three views:

- **3D**: drag to rotate, scroll to zoom.
- **Grundriss**: one storey from above, in the drawing's orientation: walls in
  their colors with doors and windows across them, slabs tinted, the roof
  dashed. Drag to move, scroll to zoom.
- **Ansicht**: one side straight on (Nord, Ost, Süd, West, taking the plan's
  y axis as north; only the names depend on that).

Storeys and the roof can be hidden. All views carry dimensions as German
building drawings do (DIN 406, DIN 1356): extension lines, oblique ticks,
meters with two decimals from 1 m and centimeters below, the millimeter
raised. With nothing selected they give the overall size of what is shown;
clicking an element highlights it and gives its own length, height and
thickness (`app/src/dimension.rs`). A selection carries over between views.

3D and Ansicht come from a software renderer (`app/src/scene.rs`): flat-shaded
faces rasterized on the CPU into a depth buffer and painted as one image, the
dimensions drawn on top. The Grundriss is drawn as vectors from the wall
footprints and the slab and roof outlines (`app/src/floor_plan.rs`).

```bash
cargo run --release -p aufmass-app                                   # start window
cargo run --release -p aufmass-app -- ~/Dokumente/Aufmaß/Haus.aufmass  # a project
cargo run --release -p aufmass-app -- ~/Downloads/Planeingang_17.09.2026 --tab EG
```

Plans on the command line open in a window without a project; `--tab <storey>`
opens that plan tab after loading, for quick checks.

The app icon is the A of Inter SemiBold drawn as a measured part on blueprint
paper. `app/assets/icon.py` writes `icon.svg` and `icon-small.svg` (16 to 32
px); `icon.ico` (embedded into the .exe by `app/build.rs`) and
`icon-macos.png` (the Dock icon) are rendered from them, see the script.

## Error reports

Both binaries report crashes and failed conversions (no 3D model, unreadable
files or solids, results not saved) to Sentry (`src/telemetry.rs`). Reports
carry error messages, file names and counts; plan folders, the home folder and
the computer name are scrubbed, and plan files never leave the PC.

The DSN is compiled in from `SENTRY_DSN` at build time (CI secret). Without one,
reporting is off; at run time `SENTRY_DSN` overrides it, and an empty value turns
it off (for local testing: `SENTRY_DSN=http://public@127.0.0.1:PORT/1` and a
small HTTP server that prints the envelopes).

## Builds

`.github/workflows/build.yml` builds and tests both binaries on macOS and
Windows and uploads them with `aufmass.toml` as artifacts. It runs when
started by hand (Actions -> Build -> Run workflow) or for a release tag. Secrets `SENTRY_DSN` and
`SENTRY_AUTH_TOKEN` and variables `SENTRY_ORG` and `SENTRY_PROJECT` turn on
error reports and the upload of debug symbols (.pdb/.dSYM) to Sentry. The Windows build has
to run on Windows, because GPUI compiles its DirectX shaders with the Windows
SDK. On macOS the app uses GPUI's `runtime_shaders`, so no Xcode Metal
toolchain is needed to build it. `.cargo/config.toml` links the C runtime
statically on Windows, so the .exe needs no Visual C++ Redistributable. The
package includes `packaging/LIESMICH.txt`, a German quick start for users.

## Releases and updates

A release is the version in `Cargo.toml` (`[workspace.package]`, shared by
both binaries) plus a matching tag:

```bash
git tag v0.2.0 && git push origin v0.2.0
```

The build then zips each platform, signs the zips with
[zipsign](https://github.com/Kijewski/zipsign) (secret `ZIPSIGN_KEY`, the
private key in base64) and publishes them as a GitHub Release. The file names
never change, so this link always gives the newest Windows version:
`https://github.com/DoktorDaveJoos/aufmass/releases/latest/download/aufmass-windows-x64.zip`.

The app checks the latest release on start (release builds only; debug builds
ask a test server given as `AUFMASS_RELEASES_API`) and offers the update in a bar; `aufmass --update` does the same from the command line
(`src/update.rs`). The zip's signature must match `packaging/update-key.pub`,
with the file name and version as context, so neither a tampered zip nor an
older release under a new tag gets installed. Both programs, LIESMICH.txt and,
when missing, aufmass.toml are replaced; an existing aufmass.toml is kept.
Downloads by the program itself carry no "from the internet" mark, so
SmartScreen only warns on the first install.

GitHub serves releases only from public repositories. `AUFMASS_RELEASES=owner/name`
at build time points the updater at another repository, e.g. a public one
holding only the releases.

The private key is not in the repository. To set up a new one:

```bash
zipsign gen-key update-signing.key packaging/update-key.pub
base64 -i update-signing.key | gh secret set ZIPSIGN_KEY
```

A new key means the installed programs reject updates signed with it: they
have to be replaced once by hand.

## Development

```bash
cargo test --workspace
cargo run --release -- ~/Downloads/Planeingang_17.09.2026 -o /tmp/out
```

## License

Proprietary, all rights reserved by bauzertifikate.de; see [LICENSE](LICENSE).
The source is visible but may not be copied, modified, built or distributed.
Anyone may run the unmodified programs from the Releases page. The release
zips include the LICENSE file.
