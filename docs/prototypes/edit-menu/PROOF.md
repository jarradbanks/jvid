# Menus and Command Palette Prototype

Open `index.html` directly. The preview is open in Orca beside the chat.

| Surface           | Approved behaviour                         |
| ----------------- | ------------------------------------------ |
| Edit              | 15 direct rows and six submenus            |
| Other menus       | 36 menu surfaces based on current app code |
| Command Palette   | 720px wide, 560px high, 40px rows          |
| Selection         | Background highlight; no left accent strip |
| Shortcut          | Ctrl+K opens the palette                   |
| Split at Playhead | Available in menus; no default shortcut    |

The palette and result area shrink to the viewport; search and footer remain
visible while results scroll. Search covers the app's 115 registered actions,
aliases and shortcuts. Escape restores focus. Empty and disabled results are
represented. Preferences retains the working preview theme picker.

Browse Menus reaches the app bar menus, panel menus and context menus. Example
projects, bins, workspaces and exports are illustrative. The menu labels and
ordering come from `crates/jvid-app/src/` and its English catalog. Field popup
controls are schematic; this is an interaction study rather than a native editor.
Other menus retain their current grouping for review. Only Edit and the palette
have approved redesigns. Actions report their selection without writing files.

## PROOF

Browser-only verification, using the existing Playwright installation:

```bash
export NODE_PATH=/home/jarrad/.npm/_npx/7f4967a1621aa3dc/node_modules
node docs/prototypes/edit-menu/check.cjs
```

Result: `PASSED` at 1280×720 and 1920×1080, each at 1.0 and 1.5 scale.
Checks cover every menu surface, nested submenus, bounds, labels, theme changes,
palette row height, Ctrl+K, unassigned Split, search, last-result scrolling,
empty results, disabled Enter, dismissal, and Export queue states.

The official polish command for each state is:

```bash
PROTOTYPE_STATE=palette node /mnt/c/Users/Jarrad/.agents/skills/polish/polish-check.js \
  --url file:///mnt/c/Users/Jarrad/orca/projects/jvid/docs/prototypes/edit-menu/index.html \
  --setup docs/prototypes/edit-menu/check.cjs \
  --widths 853,1280,1920 --height 480
```

Result: `PASSED`. The same command was run across 53 states: menu surfaces,
Edit groupings, all submenus, queue states/confirmation, and palette default,
search, empty, long-label, disabled and bottom-of-list states. The palette check
was rerun after removing its selection accent and passed. No selectors were
ignored. REVIEW notices for long existing command labels/shortcut combinations
are accepted because the current app wording is intentionally preserved.

The approved palette was also inspected in Orca's browser. The Inter font and
license are copied from the existing j-ui assets. No dependency was added.

Native implementation is delegated separately. Earlier Rust baseline checks
passed before the draft native changes; those results do not validate that
uncompleted draft. The unnecessary prototype-stage native GPU run was stopped.
