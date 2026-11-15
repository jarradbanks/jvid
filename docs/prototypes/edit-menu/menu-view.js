const chevron =
    '<svg viewBox="0 0 24 24" aria-hidden="true"><path d="m9 18 6-6-6-6"/></svg>';
const toggles = new Set([
    "bin.favorites",
    "bin.missing",
    "timeline.linked_selection",
    "snapping",
    "linked_selection",
    "preview.proxies",
    "preview.safe_areas",
    "preview.thirds",
    "preview.center",
    "9:16 guide",
    "1:1 guide",
    "inspector.transform",
    "settings_transfer.grade",
    "inspector.effects",
    "inspector.audio",
    "inspector.text",
    "settings_transfer.animation",
]);
const radioFamilies = [
    ["bin.any_usage", "bin.used", "bin.unused"],
    ["bin.any_type", "bin.video", "bin.audio"],
    ["preview.fit", "50%", "100%", "200%"],
    ["preview.full", "½", "¼"],
    ["timeline.crossfade", "timeline.dip_to_black", "timeline.dip_to_white"],
];

function entryLabel(id) {
    return labels[id] || id;
}
function childEntries(id) {
    if (id === "Workspaces") {
        const entries = [
            "workspace_editing",
            "workspace_captions",
            "workspace_audio",
            "workspace_save",
        ];
        if (scenario !== "empty") entries.push("", "Example Workspace");
        if (mode === "grouped")
            entries.push("", "maximize_panel", "reset_layout");
        return entries;
    }
    return groups[id];
}
function isDisabled(id) {
    if (id === "menu.open_recent" && scenario === "empty") return true;
    if (id === "redo" || id === "inspector.fx_up") return true;
    if (scenario !== "empty") return false;
    const global = [
        "command_palette",
        "menu.new_project",
        "menu.open",
        "menu.save",
        "menu.save_as",
        "menu.project_settings",
        "menu.shortcuts",
        "new_project",
        "open_project",
        "save",
        "save_as",
        "collect_project",
        "import",
        "import_subtitles",
        "export_subtitles",
        "mark_in",
        "mark_out",
        "add_marker",
        "selection_tool",
        "razor_tool",
        "text_tool",
        "add_arrow",
        "add_rectangle",
        "add_callout",
        "quit",
        "preferences",
        "maximize_panel",
        "reset_layout",
        "export.show_queue",
        "timeline.add_video_track",
        "timeline.add_audio_track",
        "components.title",
        "updates.check",
        "menu.about",
        "menu.tutorials",
        "menu.transcript",
        "review_highlights",
        "show_captions",
    ];
    if (
        global.includes(id) ||
        id.startsWith("workspace_") ||
        id.startsWith("tutorial.")
    )
        return false;
    return !groups[id];
}
function radioFamily(id) {
    const family = radioFamilies.find((items) => items.includes(id));
    if (family) return family;
    if (menus[currentMenu].radio && currentMenu !== "transition")
        return menus[currentMenu].entries;
    return null;
}
function entriesFor(key) {
    if (key === "edit") return mode === "grouped" ? grouped : current;
    if (key === "export" && queueState !== "empty") {
        const entries = [
            "export.show_queue",
            "export.new",
            "",
            "export.clear_finished",
            "export.cancel_all",
        ];
        return entries;
    }
    if (key === "add-effect") return effectNames;
    return menus[key].entries;
}
function renderMenu(target, entries) {
    target.replaceChildren();
    target.classList.toggle(
        "has-checks",
        entries.some((id) => toggles.has(id) || radioFamily(id)),
    );
    for (const id of entries) {
        if (!id) {
            const separator = document.createElement("div");
            separator.setAttribute("role", "separator");
            target.append(separator);
            continue;
        }
        if (id === "framing.target" || id === "timeline.open_sequence") {
            const caption = document.createElement("div");
            caption.className = "menu-caption";
            caption.textContent = entryLabel(id);
            if (id === "framing.target")
                caption.textContent = "V1: subject.mp4";
            target.append(caption);
            continue;
        }
        const row = document.createElement("button");
        row.type = "button";
        row.dataset.action = id;
        const family = radioFamily(id);
        const checkable = toggles.has(id) || Boolean(family);
        let role = "menuitem";
        if (checkable) role = family ? "menuitemradio" : "menuitemcheckbox";
        row.setAttribute("role", role);
        if (checkable) {
            const selected =
                checks.get(id) ?? Boolean(family && family[0] === id);
            row.setAttribute("aria-checked", String(selected));
        }
        row.disabled = isDisabled(id);
        if (currentMenu === "export" && id === "export.clear_finished")
            row.disabled = queueState === "running";
        if (currentMenu === "export" && id === "export.cancel_all")
            row.disabled = queueState !== "running";
        if (row.disabled) row.title = "Unavailable in this example state.";
        const label = document.createElement("span");
        label.className = "label";
        label.textContent = entryLabel(id);
        row.append(label);
        if (groups[id]) {
            row.setAttribute("aria-haspopup", "menu");
            row.setAttribute("aria-expanded", "false");
            row.insertAdjacentHTML("beforeend", chevron);
        } else if (shortcuts[id] || id.startsWith("tutorial.")) {
            const shortcut = document.createElement("span");
            shortcut.className = "shortcut";
            shortcut.textContent = shortcuts[id] || "5 min";
            row.append(shortcut);
        }
        if (id === "preferences") row.className = "preferences-row";
        row.addEventListener("pointerenter", () => {
            const level = stack.findIndex((layer) => layer.element === target);
            if (groups[id] && !row.disabled) openSubmenu(row);
            else closeAfter(level);
        });
        row.addEventListener("click", () => {
            if (groups[id]) openSubmenu(row, true);
            else runAction(id, row);
        });
        target.append(row);
    }
}
function closeAfter(level) {
    while (stack.length > level + 1) {
        const layer = stack.pop();
        layer.parentRow?.setAttribute("aria-expanded", "false");
        if (layer.element === submenu) layer.element.hidden = true;
        else layer.element.remove();
    }
}
function closeMenu() {
    closeAfter(0);
    menu.hidden = true;
    stack.length = 0;
    anchor?.setAttribute("aria-expanded", "false");
}
function positionMenu() {
    if (menu.hidden) return;
    const bounds = anchor.getBoundingClientRect();
    menu.style.maxHeight = `${innerHeight - 8}px`;
    const left = Math.max(
        4,
        Math.min(bounds.left, innerWidth - menu.offsetWidth - 4),
    );
    let top = bounds.bottom + 4;
    if (contextPoint) top = contextPoint.y;
    if (top + menu.offsetHeight > innerHeight - 4)
        top = innerHeight - menu.offsetHeight - 4;
    menu.style.left = `${left}px`;
    if (contextPoint)
        menu.style.left = `${Math.max(4, Math.min(contextPoint.x, innerWidth - menu.offsetWidth - 4))}px`;
    menu.style.top = `${Math.max(4, top)}px`;
    for (const layer of stack.slice(1)) positionSubmenu(layer);
}
function positionSubmenu(layer) {
    const row = layer.parentRow.getBoundingClientRect();
    const parent = layer.parentRow.closest(".menu").getBoundingClientRect();
    const child = layer.element;
    child.style.maxHeight = `${innerHeight - 8}px`;
    child.style.left = "0px";
    let left = parent.right - 1;
    if (left + child.offsetWidth > innerWidth - 4)
        left = parent.left - child.offsetWidth + 1;
    child.style.left = `${Math.max(4, left)}px`;
    child.style.top = `${Math.max(4, Math.min(row.top - 5, innerHeight - child.offsetHeight - 4))}px`;
}
function openSubmenu(row, focus = false) {
    const level = stack.findIndex(
        (layer) => layer.element === row.closest(".menu"),
    );
    closeAfter(level);
    let child = submenu;
    if (level > 0) {
        child = document.createElement("div");
        child.className = "menu submenu";
        child.setAttribute("role", "menu");
        document.body.append(child);
    }
    row.setAttribute("aria-expanded", "true");
    child.setAttribute("aria-label", entryLabel(row.dataset.action));
    renderMenu(child, childEntries(row.dataset.action));
    child.hidden = false;
    const layer = { element: child, parentRow: row };
    stack.push(layer);
    positionSubmenu(layer);
    if (focus) child.querySelector("button:not(:disabled)")?.focus();
}
function addSpecialContent() {
    const special = menus[currentMenu].special;
    if (!special) return;
    const panel = document.createElement("div");
    panel.className = "menu-extra";
    if (special === "queue") {
        let heading = entryLabel("export.empty_title");
        if (queueState === "running") heading = "Rendering";
        if (queueState === "finished" || queueState === "failed")
            heading = entryLabel("export.all_finished");
        panel.innerHTML = "<strong></strong><p></p>";
        panel.querySelector("strong").textContent = heading;
        if (queueState === "running")
            panel.querySelector("p").textContent = "42% / Example Export.mp4";
        if (queueState === "finished")
            panel.querySelector("p").textContent = "Example Export.mp4 / Done";
        if (queueState === "failed")
            panel.querySelector("p").textContent =
                "Example Export.mp4 / Failed";
        if (queueState === "empty")
            panel.querySelector("p").textContent =
                "Exports you start appear here.";
    }
    if (special === "marker") {
        panel.innerHTML =
            '<label>Name<input value="Example Marker" /></label><label>Color<select aria-label="Marker Color"><option>Amber</option><option>Red</option><option>Green</option><option>Blue</option><option>Plum</option><option>Violet</option></select></label><label>Note<textarea aria-label="Marker Note" rows="3"></textarea></label>';
    }
    if (special === "easing") {
        panel.innerHTML =
            '<div class="easing-choices"></div><svg class="curve" viewBox="0 0 140 90" aria-label="Easing Curve"><path d="M4 86 C20 86 120 4 136 4"/></svg>';
        for (const id of [
            "easing.linear",
            "easing.ease_in",
            "easing.ease_out",
            "easing.ease_in_out",
            "easing.hold",
            "easing.bezier",
        ]) {
            const button = document.createElement("button");
            button.textContent = entryLabel(id);
            button.addEventListener("click", () => {
                feedback.textContent = `${entryLabel(id)} selected for this preview.`;
                panel
                    .querySelector(".curve path")
                    .setAttribute(
                        "d",
                        id === "easing.hold" ? "M4 86 H136 V4" : "M4 86 L136 4",
                    );
            });
            panel.querySelector(".easing-choices").append(button);
        }
    }
    if (special === "effects") {
        const search = document.createElement("input");
        search.type = "search";
        search.placeholder = entryLabel("inspector.search_effects");
        search.setAttribute("aria-label", search.placeholder);
        search.addEventListener("input", () => {
            for (const button of menu.querySelectorAll("[data-action]"))
                button.hidden = !button.textContent
                    .toLowerCase()
                    .includes(search.value.toLowerCase());
            positionMenu();
        });
        panel.append(search);
    }
    menu.prepend(panel);
}
