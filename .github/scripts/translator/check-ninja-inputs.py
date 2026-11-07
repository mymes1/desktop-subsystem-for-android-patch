"""Lists every input the build of the given targets needs that is neither in the tree nor
made by a rule, and fails if there is one.

  check-ninja-inputs.py --top <tree> --ninja <ninja> --file <combined.ninja>
                        [--report <file>] <target>...

Ninja stops at the first such file ("missing and no known rule to make it"), so a partial
tree would cost one build run per missing file. This asks ninja for the whole graph of the
targets (`ninja -t graph`, which every ninja has) instead, and reports every leaf of that
graph, that is every file no edge produces, that does not exist. Soong-only builds run it
after the analysis (m --skip-ninja) and before the real build.
"""

import argparse
import os
import re
import subprocess
import sys

NODE = re.compile(r'^"(0x[0-9a-f]+)" \[label="((?:[^"\\]|\\.)*)"(.*)\]$')
EDGE = re.compile(r'^"(0x[0-9a-f]+)" -> "(0x[0-9a-f]+)"')


def leaves(lines):
    """The file nodes of a `ninja -t graph` output that no edge leads into."""
    files, edge_nodes, produced = {}, set(), set()
    for line in lines:
        line = line.strip()
        m = NODE.match(line)
        if m:
            node, label, rest = m.groups()
            # Edges with several inputs or outputs are drawn as their own (ellipse) node.
            if "shape=ellipse" in rest:
                edge_nodes.add(node)
            else:
                files[node] = label.replace('\\"', '"').replace("\\\\", "\\")
            continue
        m = EDGE.match(line)
        if m:
            produced.add(m.group(2))
    return sorted(label for node, label in files.items() if node not in produced)


def missing(top, paths):
    # exists() follows links: a link into a folder the tree leaves out is as missing to ninja
    # as no file at all (the sixth run's frameworks/base/core/jni/android_media_MediaMetricsJNI.h).
    return [p for p in paths if not os.path.exists(p if os.path.isabs(p) else os.path.join(top, p))]


def main(argv):
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("--top", required=True)
    ap.add_argument("--ninja", required=True)
    ap.add_argument("--file", required=True)
    ap.add_argument("--report", help="also write the list here, for the build's log artifact")
    ap.add_argument("targets", nargs="+")
    args = ap.parse_args(argv)
    run = subprocess.run([args.ninja, "-f", args.file, "-t", "graph", *args.targets],
                         cwd=args.top, capture_output=True, text=True)
    if run.returncode != 0:
        sys.stderr.write(run.stderr)
        print("ninja could not list the graph of the targets", file=sys.stderr)
        return 1
    found = leaves(run.stdout.splitlines())
    absent = missing(args.top, found)
    lines = [f"{len(found)} files the targets are built from, {len(absent)} of them neither in "
             "the tree nor made by a rule"]
    lines += [f"missing input: {path}" for path in absent]
    if absent:
        by_folder = {}
        for path in absent:
            key = "/".join(path.split("/")[:2])
            by_folder[key] = by_folder.get(key, 0) + 1
        lines.append("by folder: " + ", ".join(f"{k} ({n})" for k, n in sorted(by_folder.items())))
    text = "\n".join(lines) + "\n"
    sys.stdout.write(text)
    if args.report:
        with open(args.report, "w", encoding="utf-8") as report:
            report.write(text)
    return 1 if absent else 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
