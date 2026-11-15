const edit = document.querySelector("#edit");
const menu = document.querySelector("#edit-menu");
const submenu = document.querySelector("#submenu");
const preferences = document.querySelector("#preferences");
const feedback = document.querySelector("#feedback");
const browse = document.querySelector("#browse");
const stack = [];
const checks = new Map([
    ["timeline.linked_selection", true],
    ["linked_selection", true],
    ["settings_transfer.animation", true],
]);
let mode = "grouped";
let currentMenu = "edit";
let scenario = "example";
let queueState = "running";
let anchor = edit;
let contextPoint = null;

function openMenu(
    key = currentMenu,
    trigger = anchor,
    focus = false,
    point = null,
) {
    closeMenu();
    currentMenu = key;
    anchor = trigger;
    contextPoint = point;
    menu.setAttribute("aria-label", menus[key].label);
    menu.style.width = "max-content";
    if (key === "edit" && mode === "grouped") menu.style.width = "252px";
    if (menus[key].special) menu.style.width = "300px";
    renderMenu(menu, entriesFor(key));
    addSpecialContent();
    menu.hidden = false;
    anchor.setAttribute("aria-expanded", "true");
    stack.push({ element: menu, parentRow: null });
    browse.value = key;
    document.querySelector("#description").textContent =
        `${menus[key].label}: current app order and grouping.`;
    if (key === "edit" && mode === "grouped")
        document.querySelector("#description").textContent =
            "Edit: approved grouping. Other menus match the current app.";
    positionMenu();
    if (focus) menu.querySelector("button:not(:disabled)")?.focus();
}
function runAction(id, row) {
    const family = radioFamily(id);
    if (toggles.has(id) || family) {
        let checked = row.getAttribute("aria-checked") !== "true";
        if (family) {
            for (const sibling of family) checks.set(sibling, false);
            checked = true;
        }
        checks.set(id, checked);
        if (family) {
            for (const sibling of row.parentElement.querySelectorAll(
                "[role=menuitemradio]",
            ))
                sibling.setAttribute("aria-checked", "false");
        }
        row.setAttribute("aria-checked", String(checked));
        feedback.textContent = `${entryLabel(id)} updated in this preview.`;
        return;
    }
    if (id === "bin.clear_filters") {
        for (const key of [
            "bin.favorites",
            "bin.missing",
            "bin.used",
            "bin.unused",
            "bin.video",
            "bin.audio",
        ])
            checks.set(key, false);
        checks.set("bin.any_usage", true);
        checks.set("bin.any_type", true);
    }
    if (id === "export.cancel_all") {
        showQueueConfirmation();
        return;
    }
    if (id === "command_palette") {
        openPalette();
        return;
    }
    const previousAnchor = anchor;
    closeMenu();
    previousAnchor.focus();
    if (id === "preferences") {
        preferences.showModal();
        return;
    }
    feedback.textContent = `Selected “${entryLabel(id)}”. Example data; no project files are changed.`;
}
function showQueueConfirmation() {
    const panel = document.createElement("div");
    panel.className = "menu-extra queue-confirmation";
    panel.innerHTML =
        '<p>Cancel 1 Export?</p><div class="queue-confirm-actions"></div>';
    for (const id of ["export.keep_exporting", "export.cancel_all"]) {
        const button = document.createElement("button");
        button.textContent = entryLabel(id);
        button.addEventListener("click", () => {
            if (id === "export.cancel_all") {
                queueState = "finished";
                document.querySelector("#queue-state").value = queueState;
                feedback.textContent = "Example export cancelled.";
            }
            openMenu("export");
        });
        panel.querySelector("div").append(button);
    }
    menu.querySelector(".queue-confirmation")?.remove();
    menu.append(panel);
    positionMenu();
}
for (const [key, definition] of Object.entries(menus)) {
    let category = [...browse.children].find(
        (group) => group.label === definition.category,
    );
    if (!category) {
        category = document.createElement("optgroup");
        category.label = definition.category;
        browse.append(category);
    }
    const option = document.createElement("option");
    option.value = key;
    option.textContent = definition.label;
    category.append(option);
}
browse.addEventListener("change", () => openMenu(browse.value, browse));
for (const trigger of document.querySelectorAll("[data-menu]")) {
    trigger.setAttribute("aria-haspopup", "menu");
    trigger.setAttribute("aria-expanded", "false");
    trigger.addEventListener("click", () => {
        if (
            !menu.hidden &&
            currentMenu === trigger.dataset.menu &&
            anchor === trigger
        )
            closeMenu();
        else openMenu(trigger.dataset.menu, trigger);
    });
}
for (const button of document.querySelectorAll("[data-mode]")) {
    button.addEventListener("click", () => {
        mode = button.dataset.mode;
        for (const option of document.querySelectorAll("[data-mode]"))
            option.setAttribute("aria-pressed", String(option === button));
        openMenu("edit", edit);
    });
}
for (const target of document.querySelectorAll("[data-context]")) {
    target.addEventListener("contextmenu", (event) => {
        if (event.target.closest("[data-context]") !== target) return;
        event.preventDefault();
        event.stopPropagation();
        openMenu(target.dataset.context, target, false, {
            x: event.clientX,
            y: event.clientY,
        });
    });
}
document.querySelector("[data-action=export]").addEventListener("click", () => {
    closeMenu();
    feedback.textContent = "Export dialog selected. This review covers menus.";
});
document.querySelector("#scenario").addEventListener("change", (event) => {
    scenario = event.target.value;
    document.body.dataset.scenario = scenario;
    document.querySelector(".project-name").textContent =
        scenario === "empty" ? "Untitled" : "Example Project";
    openMenu(currentMenu);
});
document.querySelector("#queue-state").addEventListener("change", (event) => {
    queueState = event.target.value;
    openMenu("export", document.querySelector("[data-menu=export]"));
});
document.addEventListener("pointerdown", (event) => {
    if (
        event.target.closest(
            ".menu, [data-menu], .comparison, .review-controls",
        )
    )
        return;
    closeMenu();
});
document.addEventListener("keydown", (event) => {
    if (
        document.querySelector("#command-palette").open ||
        preferences.open ||
        event.target.matches("input, textarea, select")
    )
        return;
    const activeMenu = event.target.closest("[role=menu]");
    if (event.key === "Escape") {
        event.preventDefault();
        if (stack.length > 1) {
            const row = stack.at(-1).parentRow;
            closeAfter(stack.length - 2);
            row.focus();
        } else {
            closeMenu();
            anchor.focus();
        }
        return;
    }
    if (
        event.target.dataset.menu &&
        ["ArrowDown", "Enter", " "].includes(event.key)
    ) {
        event.preventDefault();
        openMenu(event.target.dataset.menu, event.target, true);
        return;
    }
    if (!activeMenu) return;
    const rows = [
        ...activeMenu.querySelectorAll("button:not(:disabled):not([hidden])"),
    ];
    const index = rows.indexOf(document.activeElement);
    if (["ArrowDown", "ArrowUp", "Home", "End"].includes(event.key)) {
        event.preventDefault();
        let next = index + (event.key === "ArrowDown" ? 1 : -1);
        if (event.key === "Home") next = 0;
        if (event.key === "End") next = rows.length - 1;
        rows[(next + rows.length) % rows.length]?.focus();
        closeAfter(stack.findIndex((layer) => layer.element === activeMenu));
    }
    if (event.key === "ArrowRight" && groups[event.target.dataset.action]) {
        event.preventDefault();
        openSubmenu(event.target, true);
    } else if (event.key === "ArrowLeft" && stack.length > 1) {
        event.preventDefault();
        const row = stack.at(-1).parentRow;
        closeAfter(stack.length - 2);
        row.focus();
    } else if (
        ["ArrowLeft", "ArrowRight"].includes(event.key) &&
        stack.length === 1
    ) {
        const triggers = [...document.querySelectorAll(".appbar [data-menu]")];
        const index = triggers.indexOf(anchor);
        if (index < 0) return;
        event.preventDefault();
        const next = index + (event.key === "ArrowRight" ? 1 : -1);
        const trigger = triggers[(next + triggers.length) % triggers.length];
        openMenu(trigger.dataset.menu, trigger, true);
    }
    if (event.key === "Tab") closeMenu();
});
for (const button of document.querySelectorAll("button[data-theme]")) {
    button.addEventListener("click", () => {
        document.documentElement.dataset.theme = button.dataset.theme;
        for (const option of document.querySelectorAll("button[data-theme]"))
            option.setAttribute("aria-pressed", String(option === button));
        feedback.textContent = `${button.textContent.trim()} theme applied to this preview.`;
    });
}
preferences.addEventListener("click", (event) => {
    if (event.target !== preferences) return;
    const bounds = preferences.getBoundingClientRect();
    const inside =
        event.clientX >= bounds.left &&
        event.clientX <= bounds.right &&
        event.clientY >= bounds.top &&
        event.clientY <= bounds.bottom;
    if (!inside) preferences.close();
});
preferences.addEventListener("close", () => edit.focus());
menu.addEventListener("scroll", () => {
    if (stack.length < 2) return;
    const row = stack[1].parentRow.getBoundingClientRect();
    const bounds = menu.getBoundingClientRect();
    if (row.top < bounds.top || row.bottom > bounds.bottom) closeAfter(0);
    else positionMenu();
});
window.addEventListener("resize", positionMenu);
openMenu();
