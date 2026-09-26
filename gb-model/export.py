"""Export gb_model.blend to a binary glTF for the docs site's Three.js example.

Run through Blender, not a plain Python (`bpy` only exists inside it):

    blender -b gb-model/gb_model.blend --python gb-model/export.py -- <out.glb>

`just export-model` does exactly that with the right paths.
"""

import sys

import bpy

# Everything after `--` is ours; everything before it is Blender's own arguments.
argv = sys.argv[sys.argv.index("--") + 1 :] if "--" in sys.argv else []
if len(argv) != 1:
    sys.exit("usage: blender -b gb_model.blend --python export.py -- <out.glb>")
out_path = argv[0]

# Only the meshes you can see. That leaves out the camera, the light and the reference
# empties (the page brings its own camera and lights), and — the ones that matter —
# `LoudspeakerCreases` and `ModuleSlot`, which are hidden because they are the Boolean
# cutters `MainBody` subtracts. Exported, they would sit inside the body as solid
# geometry filling the very holes they cut.
bpy.ops.object.select_all(action="DESELECT")
for obj in bpy.context.view_layer.objects:
    if obj.type == "MESH" and obj.visible_get():
        obj.select_set(True)

bpy.ops.export_scene.gltf(
    filepath=out_path,
    export_format="GLB",
    use_selection=True,
    # Bake the modifiers in: without this, MainBody arrives un-bevelled and with no
    # speaker grille or cartridge slot, since those are Bevel and Boolean modifiers.
    export_apply=True,
    # Blender is Z-up, three.js is Y-up; the exporter rotates the whole scene.
    export_yup=True,
    export_cameras=False,
    export_lights=False,
)
