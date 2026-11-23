"""Render actual tool/apply output as a three-part diagram."""
import pymupdf

from manifest import ROOT, validate
from package import exchange, fixture


def path(op):
    geometry = op["geometry"]
    nodes = geometry.get("nodes")
    if nodes is None:
        nodes = [{"point": geometry[key]} for key in ("start", "end")]
    point = lambda node: tuple(node["point"][axis] for axis in ("x", "y"))

    def control(node, key):
        handle = node.get(key) or {"x": 0, "y": 0}
        return tuple(value + handle[axis] for value, axis in zip(point(node), ("x", "y")))

    commands = ["M {} {}".format(*point(nodes[0]))]
    for start, end in zip(nodes, nodes[1:]):
        if start.get("handleOut") or end.get("handleIn"):
            commands.append("C {} {} {} {} {} {}".format(
                *control(start, "handleOut"), *control(end, "handleIn"), *point(end),
            ))
        else:
            commands.append("L {} {}".format(*point(end)))
    return (f'<path d="{" ".join(commands)}" transform="translate({op["x"]} {op["y"]})" '
            'fill="none" stroke="#182735" stroke-width="2.6" stroke-linejoin="round"/>')


def part(id_, x, y):
    return {
        "id": id_, "type": "shape", "x": x, "y": y, "rotation": 0, "scale": 1,
        "bounds": {"x": x, "y": y, "width": 94, "height": 60},
        "ports": [{"x": px, "y": py} for px, py in [
            (x+47, y), (x+94, y+30), (x+47, y+60), (x, y+30),
        ]],
    }


def main():
    manifest = validate()
    binary = ROOT / "target/release" / manifest["entrypoint"]
    parts = [part("a", 85, 74), part("b", 621, 74), part("c", 392, 357)]
    frames = [fixture("initialize")]
    for index, (routing, a, a_port, b, b_port) in enumerate([
        ("straight", 0, 1, 1, 3), ("curved", 1, 2, 2, 1), ("orthogonal", 2, 3, 0, 2),
    ], 2):
        objects = [parts[a], parts[b]]
        points = [obj["ports"][port] | {"attach": {"objectId": obj["id"], "port": port}}
                  for obj, port in zip(objects, [a_port, b_port])]
        frames.append({
            "jsonrpc": "2.0", "id": str(index), "method": "tool/apply",
            "params": {"tool": "connect", "settings": {"routing": routing}, "points": points,
                       "objects": objects, "page": {"id": "preview", "widthPt": 800, "heightPt": 500}},
        })
    replies = exchange(binary, frames)
    fragments = [path(reply["result"]["batch"]["operations"][0]) for reply in replies[1:]]
    for part_ in parts:
        fragments.append(f'<rect x="{part_["x"]}" y="{part_["y"]}" width="94" height="60" rx="5" '
                         'fill="#f5f7f8" stroke="#182735" stroke-width="2.6"/>')
    svg = ('<svg xmlns="http://www.w3.org/2000/svg" width="800" height="500">'
           '<rect width="800" height="500" fill="#fffefa"/>'
           + "".join(fragments) + "</svg>")
    with pymupdf.open(stream=svg.encode(), filetype="svg") as document:
        page = document[0]
        page.get_pixmap(matrix=pymupdf.Matrix(1600/page.rect.width, 1000/page.rect.height)).save(
            ROOT / "docs/preview.png"
        )


if __name__ == "__main__":
    main()
