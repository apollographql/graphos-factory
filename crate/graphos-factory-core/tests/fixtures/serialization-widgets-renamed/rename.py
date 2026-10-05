#!/usr/bin/env python3
"""Mechanically renames every domain identifier in
../serialization-widgets/ (vendor name, type/field/argument names, path
segments, GraphQL operation names, and file/case names) via a fixed
word-substitution table, producing this directory -- same shapes, same
structure, different names, to expose accidental name coupling in
`crate::request_serialization` (task requirement: "one renamed variant to
detect name coupling").

Deliberately NOT renamed: structural/contract tokens the instrument itself
depends on by literal name -- `key`/`value` (the map-entry-input contract),
`explicit null` (the decisions.json heuristic phrase), and every WireMock/
GraphQL/Connectors protocol keyword (`request`, `response`, `method`,
`status`, `body`, `queryParams`, `selection`, `source`, `http`, `name`,
`type`, `properties`, `$ref`, `equalToJson`, `hasExactly`, etc).

Run from the repo root: python3 crate/factory-core/tests/fixtures/serialization-widgets-renamed/rename.py

NOTE: this script rebuilds its own directory from ../serialization-widgets/
on every run (`shutil.rmtree(DST)` then a fresh walk of `SRC`) and does not
copy itself -- it re-writes its own file back out at the end so a rerun
does not delete this script.
"""
import os
import re
import shutil
import sys

SRC = "crate/factory-core/tests/fixtures/serialization-widgets"
DST = "crate/factory-core/tests/fixtures/serialization-widgets-renamed"

# Ordered longest-and-most-specific first, so a compound identifier is
# substituted as a whole before its constituent shorter words are touched.
SUBSTITUTIONS = [
    (r"SearchWidgetsRequest", "SearchGizmosRequest"),
    (r"UpdateWidgetRequest", "UpdateGizmoRequest"),
    (r"Widget_Co_TagEntryInput", "Gizmo_Co_LabelEntryInput"),
    (r"Widget_Co_AddressInput", "Gizmo_Co_SiteInput"),
    (r"Widget_Co_Widget", "Gizmo_Co_Gizmo"),
    (r"Widget_Co_JSON", "Gizmo_Co_JSON"),
    (r"widget_co", "gizmo_co"),
    (r"Widget_Co", "Gizmo_Co"),
    (r"widget-co", "gizmo-co"),
    (r"listWidgets", "listGizmos"),
    (r"searchWidgets", "searchGizmos"),
    (r"updateWidget\b", "updateGizmo"),
    (r"/widgets/search", "/gizmos/search"),
    (r"/widgets/\{\$args\.id\}", "/gizmos/{$args.id}"),
    (r"/widgets/\{id\}", "/gizmos/{id}"),
    (r"/widgets/w1", "/gizmos/w1"),
    (r"/widgets\b", "/gizmos"),
    # Deliberately no `\b` boundary on these two: Python's `\b` treats `_`
    # as a word character, so it would not match "widgets"/"widget" inside
    # a snake_case case/file name like "update_widget_full" or a log
    # line's case reference "PASS: list_widgets" -- exactly the mismatch
    # this rename script exists to avoid (found when the log's stale case
    # names caused every obligation to read as unproven against the
    # correctly-renamed case/mapping *files*).
    (r"widgets", "gizmos"),
    (r"widget", "gizmo"),
    (r"\bWidget\b", "Gizmo"),
    (r"\bAddress\b", "Site"),
    (r"\baddress\b", "site"),
    (r"\bnote\b", "memo"),
    (r"\bcity\b", "town"),
    (r"\bzip\b", "postal"),
    (r"\bterm\b", "phrase"),
    (r"\bids\b", "refs"),
    (r"\btags\b", "labels"),
    (r"\bunits\b", "measure"),
    (r"\bamount\b", "qty"),
    (r"\bcolor\b", "hue"),
]

FILENAME_SUBSTITUTIONS = [
    (r"widget-co", "gizmo-co"),
    (r"list_widgets", "list_gizmos"),
    (r"search_widgets", "search_gizmos"),
    (r"update_widget", "update_gizmo"),
]

SELF_CONTENT = None  # filled in from __file__ at run time


def substitute(text: str) -> str:
    for pattern, repl in SUBSTITUTIONS:
        text = re.sub(pattern, repl, text)
    return text


def substitute_name(name: str) -> str:
    for pattern, repl in FILENAME_SUBSTITUTIONS:
        name = re.sub(pattern, repl, name)
    return name


def main():
    with open(__file__, "r") as f:
        self_content = f.read()
    readme_path = os.path.join(DST, "README.md")
    readme_content = None
    if os.path.exists(readme_path):
        with open(readme_path, "r") as f:
            readme_content = f.read()
    if os.path.exists(DST):
        shutil.rmtree(DST)
    for dirpath, _dirnames, filenames in os.walk(SRC):
        rel_dir = os.path.relpath(dirpath, SRC)
        for fname in filenames:
            if fname == "README.md":
                # This directory's own README is hand-authored (it
                # describes the rename relationship itself); a blind
                # substitution pass would corrupt its meta-references to
                # the two directory names, so it is never overwritten here.
                continue
            src_path = os.path.join(dirpath, fname)
            new_fname = substitute_name(fname)
            dst_dir = os.path.join(DST, rel_dir) if rel_dir != "." else DST
            os.makedirs(dst_dir, exist_ok=True)
            dst_path = os.path.join(dst_dir, new_fname)
            with open(src_path, "r") as f:
                text = f.read()
            with open(dst_path, "w") as f:
                f.write(substitute(text))
    with open(os.path.join(DST, "rename.py"), "w") as f:
        f.write(self_content)
    if readme_content is not None:
        with open(readme_path, "w") as f:
            f.write(readme_content)
    print(f"wrote {DST}")


if __name__ == "__main__":
    sys.exit(main())
