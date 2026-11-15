const palette = document.querySelector("#command-palette");
const paletteQuery = document.querySelector("#palette-query");
const paletteResults = document.querySelector("#palette-results");
let selectedCommand = 0;
let paletteMatches = [];
let palettePreviousFocus = null;

function commandCategory(id) {
    if (
        id.startsWith("workspace_") ||
        ["maximize_panel", "reset_layout"].includes(id)
    )
        return "Workspace";
    if (id.startsWith("focus_") || id === "command_palette")
        return "Navigation";
    if (
        [
            "new_project",
            "open_project",
            "save",
            "save_as",
            "collect_project",
            "import",
            "import_subtitles",
            "export",
            "export_versions",
            "export_highlight_versions",
            "quit",
        ].includes(id)
    )
        return "Project";
    if (
        [
            "play_stop",
            "shuttle_left",
            "shuttle_stop",
            "shuttle_right",
            "loop_playback",
            "audio_scrub",
            "step_back",
            "step_forward",
            "step_back_second",
            "step_forward_second",
            "go_to_start",
            "go_to_end",
        ].includes(id)
    )
        return "Playback";
    if (
        [
            "record",
            "clean_up_voice",
            "detect_silence",
            "review_speech_cuts",
        ].includes(id)
    )
        return "Audio";
    if (id.startsWith("media_")) return "Media";
    if (
        id.startsWith("add_") &&
        id !== "add_edit" &&
        id !== "add_marker" &&
        id !== "add_range_marker"
    )
        return "Annotations";
    return "Timeline";
}
function commandReason(id) {
    if (!isDisabled(id)) return "";
    if (id === "redo") return "Nothing to redo.";
    if (
        [
            "cut",
            "copy",
            "duplicate",
            "delete",
            "ripple_delete",
            "copy_settings",
            "paste_settings",
            "select_all",
            "link",
        ].includes(id)
    )
        return "Select clips on the timeline first.";
    if (
        [
            "export",
            "export_versions",
            "export_highlight_versions",
            "export_frame",
        ].includes(id)
    )
        return "Add content to the timeline first.";
    return labels["palette.unavailable"];
}
function renderPalette() {
    const words = paletteQuery.value
        .trim()
        .toLowerCase()
        .split(/\s+/)
        .filter(Boolean);
    paletteMatches = paletteActions.filter((id) => {
        const search = [
            entryLabel(id),
            id.replaceAll("_", " "),
            shortcuts[id] || "",
            paletteAliases[id] || "",
        ]
            .join(" ")
            .toLowerCase();
        return words.every((word) => search.includes(word));
    });
    selectedCommand = Math.min(
        selectedCommand,
        Math.max(0, paletteMatches.length - 1),
    );
    paletteResults.replaceChildren();
    if (!paletteMatches.length) {
        const message = document.createElement("div");
        message.className = "palette-no-results";
        message.innerHTML =
            "<h3>No Matching Commands</h3><p>Try an action name or a shortcut.</p>";
        paletteResults.append(message);
        paletteQuery.removeAttribute("aria-activedescendant");
        return;
    }
    for (const [index, id] of paletteMatches.entries()) {
        const row = document.createElement("div");
        row.className = "palette-result";
        row.id = `command-${id}`;
        row.dataset.command = id;
        row.setAttribute("role", "option");
        row.setAttribute("aria-selected", String(index === selectedCommand));
        const reason = commandReason(id);
        row.setAttribute("aria-disabled", String(Boolean(reason)));
        row.tabIndex = -1;
        const copy = document.createElement("span");
        copy.className = "command-copy";
        const name = document.createElement("span");
        name.className = "command-name";
        name.textContent = entryLabel(id);
        const detail = document.createElement("span");
        detail.className = "command-detail";
        detail.textContent = reason || commandCategory(id);
        copy.append(name, detail);
        row.append(copy);
        if (shortcuts[id]) {
            const keys = document.createElement("span");
            keys.className = "command-keys";
            for (const part of shortcuts[id].split("+")) {
                const key = document.createElement("kbd");
                key.textContent = part;
                keys.append(key);
            }
            row.append(keys);
        }
        row.addEventListener("pointermove", () => selectCommand(index, false));
        row.addEventListener("pointerdown", (event) => event.preventDefault());
        row.addEventListener("click", () => executeCommand(id));
        paletteResults.append(row);
    }
    selectCommand(selectedCommand, true);
}
function selectCommand(index, scroll) {
    selectedCommand = index;
    for (const [rowIndex, row] of [
        ...paletteResults.querySelectorAll("[role=option]"),
    ].entries())
        row.setAttribute("aria-selected", String(rowIndex === index));
    const selected = paletteResults.querySelector("[aria-selected=true]");
    if (!selected) return;
    paletteQuery.setAttribute("aria-activedescendant", selected.id);
    if (scroll) selected.scrollIntoView({ block: "nearest" });
    const context = document.querySelector("#palette-context");
    context.textContent = commandCategory(paletteMatches[index]);
}
function openPalette() {
    palettePreviousFocus = document.activeElement;
    closeMenu();
    if (preferences.open) preferences.close();
    paletteQuery.value = "";
    selectedCommand = 0;
    if (!palette.open) palette.showModal();
    renderPalette();
    paletteQuery.focus();
}
function executeCommand(id) {
    const reason = commandReason(id);
    if (reason) {
        feedback.textContent = reason;
        return;
    }
    palette.close();
    if (id === "command_palette") {
        openPalette();
        return;
    }
    feedback.textContent = `Selected “${entryLabel(id)}” from the palette. Example data only.`;
}
paletteQuery.addEventListener("input", () => {
    selectedCommand = 0;
    renderPalette();
});
paletteQuery.addEventListener("keydown", (event) => {
    if (["ArrowDown", "ArrowUp", "Home", "End"].includes(event.key)) {
        event.preventDefault();
        if (!paletteMatches.length) return;
        let next = selectedCommand + (event.key === "ArrowDown" ? 1 : -1);
        if (event.key === "Home") next = 0;
        if (event.key === "End") next = paletteMatches.length - 1;
        selectCommand(
            (next + paletteMatches.length) % paletteMatches.length,
            true,
        );
    }
    if (event.key === "Enter") {
        event.preventDefault();
        if (paletteMatches.length)
            executeCommand(paletteMatches[selectedCommand]);
    }
});
document.addEventListener(
    "keydown",
    (event) => {
        if (
            !(event.ctrlKey || event.metaKey) ||
            event.key.toLowerCase() !== "k"
        )
            return;
        event.preventDefault();
        event.stopPropagation();
        if (palette.open) palette.close();
        else openPalette();
    },
    true,
);
document
    .querySelector("#palette-trigger")
    .addEventListener("click", openPalette);
document
    .querySelector("#palette-close")
    .addEventListener("click", () => palette.close());
palette.addEventListener("click", (event) => {
    if (event.target !== palette) return;
    const bounds = palette.getBoundingClientRect();
    const inside =
        event.clientX >= bounds.left &&
        event.clientX <= bounds.right &&
        event.clientY >= bounds.top &&
        event.clientY <= bounds.bottom;
    if (!inside) palette.close();
});
palette.addEventListener("close", () => {
    if (
        palettePreviousFocus?.isConnected &&
        palettePreviousFocus.getClientRects().length
    )
        palettePreviousFocus.focus();
    else edit.focus();
});
