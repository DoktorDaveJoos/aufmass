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

The desktop app has two hand-over buttons. Set the program paths in the config:

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

`app/` is a GPUI desktop app (`aufmass-app`) on top of the same library: drag a
folder or DWG/DXF files onto the window, or use "Ordner wählen …" / "Dateien
wählen …". It writes the same output folder and shows the quantities with
subtotals per element type (for the whole building or one storey), the plan
previews per storey and a 3D view (drag to rotate, scroll to zoom, storeys and
roof can be hidden).

The 3D view is a software renderer: the faces of each solid, back faces
culled, drawn back to front per element. Good enough for a few thousand faces;
it has no z-buffer, so intersecting elements can overlap wrongly.

```bash
cargo run --release -p aufmass-app                                   # empty window
cargo run --release -p aufmass-app -- ~/Downloads/Planeingang_17.09.2026 --tab EG
```

`--tab <storey>` opens that plan tab after loading, for quick checks.

## Builds

`.github/workflows/build.yml` builds and tests both binaries on macOS and
Windows and uploads them with `aufmass.toml` as artifacts. The Windows build has
to run on Windows, because GPUI compiles its DirectX shaders with the Windows
SDK. On macOS the app uses GPUI's `runtime_shaders`, so no Xcode Metal
toolchain is needed to build it. `.cargo/config.toml` links the C runtime
statically on Windows, so the .exe needs no Visual C++ Redistributable. The
package includes `packaging/LIESMICH.txt`, a German quick start for users.

## Development

```bash
cargo test --workspace
cargo run --release -- ~/Downloads/Planeingang_17.09.2026 -o /tmp/out
```
