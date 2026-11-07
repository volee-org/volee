// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.
//
// Adapted from mdbook-mermaid's mermaid-init.js for volee's Catppuccin
// themes: Latte is light, Frappe, Macchiato and Mocha are dark. The original
// looked for mdBook's own theme buttons (ayu, navy, coal...), which this book
// does not have, and drew dark diagrams on dark pages.

(() => {
    const darkThemes = ['frappe', 'macchiato', 'mocha', 'ayu', 'navy', 'coal'];
    const lightThemes = ['latte', 'light', 'rust'];

    const classList = document.documentElement.classList;
    const pageIsDark = darkThemes.some((theme) => classList.contains(theme));

    mermaid.initialize({ startOnLoad: true, theme: pageIsDark ? 'dark' : 'default' });

    // Mermaid draws once; switching between a light and a dark theme reloads
    // the page so that diagrams follow.
    const watch = (themes, reloadWhenDark) => {
        for (const theme of themes) {
            const button = document.getElementById('mdbook-theme-' + theme);
            if (button) {
                button.addEventListener('click', () => {
                    if (pageIsDark === reloadWhenDark) {
                        window.location.reload();
                    }
                });
            }
        }
    };
    watch(darkThemes, false);
    watch(lightThemes, true);
})();
