# dmgbuild settings for the Iki installer window.
#   dmgbuild -s scripts/dmg-settings.py -D app=dist/Iki.app Iki dist/Iki-<version>.dmg
# Icon centres must match appX, appsX and iconY in make-dmg-background.swift.
import os.path

app = defines["app"]  # noqa: F821  (dmgbuild injects `defines`)
name = os.path.basename(app)

format = "UDZO"
filesystem = "HFS+"
files = [app]
symlinks = {"Applications": "/Applications"}
icon = os.path.join(app, "Contents/Resources/AppIcon.icns")

background = "assets/dmg/background.png"
window_rect = ((200, 160), (640, 400))
default_view = "icon-view"
show_status_bar = False
show_tab_view = False
show_toolbar = False
show_pathbar = False
show_sidebar = False
icon_size = 112
text_size = 13
icon_locations = {name: (170, 205), "Applications": (470, 205)}
