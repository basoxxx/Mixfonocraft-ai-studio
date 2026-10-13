# Impostazioni dmgbuild per il DMG di Mixfonocraft AI Studio.
#
# Uso (dalla radice del repository):
#   dmgbuild -s packaging/macos/dmg_settings.py \
#     -D app_path=dist/stage/Mixfonocraft.app \
#     -D volume_icon=assets/icon/icon.icns \
#     -D background=assets/dmg/background.png \
#     "Mixfonocraft AI Studio" dist/Mixfonocraft-0.1.0-macos-arm64.dmg
#
# dmgbuild scrive il layout della finestra (icone, sfondo, dimensioni) senza
# bisogno di una sessione Finder, quindi funziona su runner CI senza GUI.

import os

# Parametri passati con -D. Con i default relativi alla radice del repository.
_app_path = defines.get("app_path", "dist/stage/Mixfonocraft.app")  # noqa: F821
_app_name = os.path.basename(os.path.normpath(_app_path))

format = "UDZO"
filesystem = "HFS+"

files = [_app_path]
symlinks = {"Applications": "/Applications"}

# Icona del volume (quella montata sulla scrivania).
icon = defines.get("volume_icon", "assets/icon/icon.icns")  # noqa: F821

# Sfondo: PNG 1x (660x400). Il 2x non viene usato per robustezza.
background = defines.get("background", "assets/dmg/background.png")  # noqa: F821

show_status_bar = False
show_tab_view = False
show_toolbar = False
show_pathbar = False
show_sidebar = False
sidebar_width = 180

# Finestra 660x400 (origine 100,100 sullo schermo).
window_rect = ((100, 100), (660, 400))
default_view = "icon-view"
show_icon_preview = False
include_icon_view_settings = "auto"
include_list_view_settings = False

arrange_by = None
grid_offset = (0, 0)
grid_spacing = 100
scroll_position = (0, 0)
label_pos = "bottom"
text_size = 13
icon_size = 128

# Nasconde l'estensione .app nel Finder.
hide_extensions = [_app_name]

# Centri delle icone (coordinate della finestra, come nel background).
icon_locations = {
    _app_name: (170, 200),
    "Applications": (490, 200),
}
