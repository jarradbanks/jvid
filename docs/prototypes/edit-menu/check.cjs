const assert = require("node:assert/strict");
const path = require("node:path");
const fs = require("node:fs");
const { pathToFileURL } = require("node:url");
const url = pathToFileURL(path.join(__dirname, "index.html")).href;

async function setup(page) {
    const state = process.env.PROTOTYPE_STATE || "grouped";
    if (state.startsWith("menu:"))
        await page.locator("#browse").selectOption(state.slice(5));
    if (state === "closed") await page.locator("#edit").click();
    if (state === "current") await page.locator("[data-mode=current]").click();
    if (
        [
            "Clip Edits",
            "In/Out",
            "Markers",
            "Annotations",
            "Clip Settings",
            "Workspaces",
        ].includes(state)
    )
        await page.locator(`#edit-menu [data-action="${state}"]`).hover();
    if (state.startsWith("theme:")) {
        await page.locator("[data-action=preferences]").click();
        await page.locator(`button[data-theme="${state.slice(6)}"]`).click();
    }
    if (state.startsWith("queue:")) {
        await page
            .locator("#queue-state")
            .selectOption(
                state.slice(6) === "confirm" ? "running" : state.slice(6),
            );
        if (state === "queue:confirm")
            await page
                .locator('#edit-menu [data-action="export.cancel_all"]')
                .click();
    }
    if (state.startsWith("palette")) {
        if (state === "palette:disabled")
            await page.locator("#scenario").selectOption("empty");
        await page.keyboard.press("Control+k");
        if (state === "palette:search" || state === "palette:disabled")
            await page.locator("#palette-query").fill("export");
        if (state === "palette:empty")
            await page.locator("#palette-query").fill("no-such-command-xyz");
        if (state === "palette:long")
            await page.locator("#palette-query").fill("ripple");
        if (state === "palette:bottom") await page.keyboard.press("End");
    }
    await page.evaluate(() => document.fonts.ready);
}
async function assertBounds(page) {
    const geometry = await page
        .locator("[role=menu]:visible")
        .evaluateAll((menus) =>
            menus.map((menu) => {
                const bounds = menu.getBoundingClientRect();
                const result = {
                    left: bounds.left,
                    top: bounds.top,
                    right: bounds.right,
                    bottom: bounds.bottom,
                    width: innerWidth,
                    height: innerHeight,
                    sideways: menu.scrollWidth > menu.clientWidth,
                };
                return result;
            }),
        );
    for (const menu of geometry) {
        assert(
            menu.left >= 0 &&
                menu.top >= 0 &&
                menu.right <= menu.width + 1 &&
                menu.bottom <= menu.height + 1,
            "Menus stay inside the viewport",
        );
        assert(!menu.sideways, "Menus do not scroll horizontally");
    }
}
async function verify() {
    const { chromium } = require("playwright");
    const browser = await chromium.launch();
    try {
        for (const [width, height, scale] of [
            [1280, 720, 1],
            [1920, 1080, 1],
            [1280, 720, 1.5],
            [1920, 1080, 1.5],
        ]) {
            const context = await browser.newContext({
                viewport: {
                    width: Math.floor(width / scale),
                    height: Math.floor(height / scale),
                },
                deviceScaleFactor: scale,
            });
            const page = await context.newPage();
            const errors = [];
            page.on("pageerror", (error) => errors.push(error.message));
            await page.goto(url);
            await page.evaluate(() => document.fonts.ready);
            const keys = await page
                .locator("#browse option")
                .evaluateAll((options) =>
                    options.map((option) => option.value),
                );
            assert.equal(keys.length, 36);
            for (const key of keys) {
                await page.locator("#browse").selectOption(key);
                await assertBounds(page);
                assert(
                    await page.locator("#edit-menu").isVisible(),
                    key + " opens",
                );
                const missing = await page
                    .locator("#edit-menu [data-action]")
                    .evaluateAll((rows) =>
                        rows
                            .filter((row) =>
                                /^[a-z_]+\.[a-z_]+$|^[a-z]+_[a-z_]+$/.test(
                                    row.querySelector(".label").textContent,
                                ),
                            )
                            .map((row) => row.textContent),
                    );
                assert.deepEqual(missing, [], key + " resolves every label");
            }
            await page.locator("#edit").click();
            if (!(await page.locator("#edit-menu").isVisible()))
                await page.locator("#edit").click();
            await page.locator("#edit-menu [data-action=Workspaces]").hover();
            await page
                .locator('#submenu [data-action="Example Workspace"]')
                .hover();
            assert.equal(
                await page.locator("[role=menu]:visible").count(),
                3,
                "Nested saved workspace opens",
            );
            await assertBounds(page);
            await page.keyboard.press("Escape");
            await page.keyboard.press("Escape");
            await page.locator("#edit-menu [data-action=preferences]").click();
            await page.locator("button[data-theme=light]").click();
            assert.equal(
                await page.locator("html").getAttribute("data-theme"),
                "light",
            );
            await page.locator("button[data-theme=dark]").click();
            await page.keyboard.press("Escape");
            await page.keyboard.press("Control+k");
            assert(await page.locator("#command-palette").isVisible());
            const paletteGeometry = await page
                .locator("#command-palette")
                .evaluate((dialog) => {
                    const bounds = dialog.getBoundingClientRect();
                    const row = dialog
                        .querySelector("[role=option]")
                        .getBoundingClientRect();
                    const result = {
                        top: bounds.top,
                        bottom: bounds.bottom,
                        height: innerHeight,
                        rowHeight: row.height,
                        count: dialog.querySelectorAll("[role=option]").length,
                    };
                    return result;
                });
            assert.equal(paletteGeometry.rowHeight, 40);
            assert(
                paletteGeometry.top >= 0 &&
                    paletteGeometry.bottom <= paletteGeometry.height,
            );
            assert.equal(paletteGeometry.count, 115);
            await page.locator("#palette-query").fill("Ctrl+K");
            assert.equal(
                await page.locator("#palette-results [role=option]").count(),
                1,
            );
            assert.equal(
                await page
                    .locator("#palette-results [role=option]")
                    .getAttribute("data-command"),
                "command_palette",
            );
            await page.locator("#palette-query").fill("split");
            assert.equal(
                await page.locator("[data-command=add_edit] kbd").count(),
                0,
                "Split stays unassigned",
            );
            await page.locator("#palette-query").fill("");
            await page.keyboard.press("End");
            const selectedVisible = await page
                .locator("#palette-results [aria-selected=true]")
                .evaluate((row) => {
                    const bounds = row.getBoundingClientRect();
                    const viewport = row.parentElement.getBoundingClientRect();
                    const visible =
                        bounds.top >= viewport.top &&
                        bounds.bottom <= viewport.bottom;
                    return visible;
                });
            assert(
                selectedVisible,
                "Keyboard selection scrolls fully into view",
            );
            await page.locator("#palette-query").fill("no-such-command-xyz");
            assert.equal(
                await page.locator("#palette-results [role=option]").count(),
                0,
            );
            await page.keyboard.press("Enter");
            assert(await page.locator("#command-palette").isVisible());
            await page.keyboard.press("Escape");
            await page.locator("#scenario").selectOption("empty");
            await page.keyboard.press("Control+k");
            await page.locator("#palette-query").fill("export media");
            assert.equal(
                await page
                    .locator("[data-command=export]")
                    .getAttribute("aria-disabled"),
                "true",
            );
            await page.keyboard.press("Enter");
            assert(await page.locator("#command-palette").isVisible());
            await page.keyboard.press("Control+k");
            assert(!(await page.locator("#command-palette").isVisible()));
            await page.locator("#scenario").selectOption("example");
            for (const state of ["empty", "running", "finished", "failed"]) {
                await page.locator("#queue-state").selectOption(state);
                await assertBounds(page);
            }
            await page.locator("#queue-state").selectOption("running");
            await page
                .locator('#edit-menu [data-action="export.cancel_all"]')
                .click();
            assert(await page.locator(".queue-confirmation").isVisible());
            await assertBounds(page);
            assert.deepEqual(errors, []);
            await context.close();
            console.log(
                `PASS ${width}×${height} at ${scale}: menus, nested submenus, themes, palette search, shortcuts, scrolling, disabled/empty states, queue`,
            );
        }
        console.log("PASSED");
    } finally {
        await browser.close();
    }
}
module.exports = setup;
if (require.main === module)
    verify().catch((error) => {
        console.error(error);
        process.exitCode = 1;
    });
