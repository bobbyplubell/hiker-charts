"""Single source of truth for the Rust source roots governed by the repo's
structural lint scripts (`check-lengths.py` and `check-splits.py`).

Adapted from the parent Hiker repo (`../notes/scripts/_rust_roots.py`) for the
`hiker-charts` submodule's crate layout. The two structural checks share this
list so a crate is either governed by both or neither.

To bring a crate under the file-length cap AND the anti-split checks, add its
`src` dir here. Editing this list is a deliberate posture change, not an agent's
escape hatch.
"""

from __future__ import annotations

RUST_ROOTS = [
    "core/src",
    "backend-plotters/src",
    "cli/src",
    "gui/src",
]
