"""cadwork 3d plugin: builds walls, slabs and roofs from an Aufmaß building.json.

Takes the file the Aufmaß app last wrote (named in %APPDATA%\\aufmass\\letzte-auswertung.txt),
else asks for one. Every element becomes a polygon panel: the reference outline
from building.json, extruded by the element thickness along its normal, with
window holes cut out. Name = element name, group = storey, subgroup = element
type, comment = Aufmaß ID.

building.json is in meters, cadwork in millimeters.
"""

import json
import os

import attribute_controller as ac
import cadwork
import element_controller as ec
import utility_controller as uc

MM = 1000.0
KIND_LABEL = {"wall": "Wand", "slab": "Decke", "roof": "Dach"}
# Window cutters reach this far past both faces, so rounding never leaves a skin.
CUT_OVERLAP = 0.01


def last_output():
    pointer = os.path.join(os.environ.get("APPDATA", ""), "aufmass", "letzte-auswertung.txt")
    try:
        with open(pointer, encoding="utf-8") as f:
            path = f.read().strip()
    except OSError:
        return None
    return path if os.path.isfile(path) else None


def point(p, shift=(0.0, 0.0, 0.0), k=0.0):
    return cadwork.point_3d((p[0] + shift[0] * k) * MM, (p[1] + shift[1] * k) * MM, (p[2] + shift[2] * k) * MM)


def vec(v):
    return cadwork.point_3d(v[0], v[1], v[2])


def panel(loop, thickness, normal, axis, start_offset=0.0):
    """Extrudes `loop` (meters) by `thickness` along `normal`, starting `start_offset` along it."""
    vertices = cadwork.vertex_list()
    for p in loop:
        vertices.append(point(p, normal, start_offset))
    vertices.append(point(loop[0], normal, start_offset))
    # Extrusion runs along the x direction; z only orients the panel.
    return ec.create_polygon_panel(vertices, thickness * MM, vec(normal), vec(axis))


def build(building):
    created = []
    for storey in building["storeys"]:
        for e in storey["elements"]:
            if len(e["outline"]) < 3:
                continue
            pieces = [panel(e["outline"], e["thickness"], e["normal"], e["axis"])]
            cutters = [
                panel(h, e["thickness"] + 2 * CUT_OVERLAP, e["normal"], e["axis"], -CUT_OVERLAP)
                for h in e["holes"]
                if len(h) >= 3
            ]
            if cutters:
                # A hole can split the panel; the extra pieces come back as new IDs.
                pieces.extend(ec.subtract_elements(cutters, pieces[:1]))
                ec.delete_elements(cutters)
            ac.set_name(pieces, e["name"])
            ac.set_group(pieces, storey["name"])
            ac.set_subgroup(pieces, KIND_LABEL.get(e["kind"], e["kind"]))
            ac.set_comment(pieces, e["id"])
            created.extend(pieces)
    return created


def main():
    path = last_output()
    if path and not uc.get_user_bool("Letzte Auswertung importieren?\n" + path, True):
        path = None
    if not path:
        path = uc.get_user_file_from_dialog("building.json (*.json)")
    if not path:
        return
    try:
        with open(path, encoding="utf-8") as f:
            building = json.load(f)
    except (OSError, ValueError) as err:
        uc.print_error("building.json konnte nicht gelesen werden: %s" % err)
        return
    created = build(building)
    ec.add_created_elements_to_undo(created)
    uc.print_to_console("Aufmaß: %d Bauteile aus %s erzeugt" % (len(created), path))


main()
