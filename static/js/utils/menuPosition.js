// @ts-check

/**
 * Single positioning engine for every floating menu in the app —
 * file/folder context menus, the My Shares per-row action menu, the
 * batch-toolbar "more" menu, and any future overlay that needs to sit
 * near a trigger button or a click point.
 *
 * Replaces a sprawl of ad-hoc `style.top`/`style.left` formulas, none of
 * which agreed on viewport clamping. The recurring bug fixed here:
 * triggers near the bottom of the screen produced menus that overflowed
 * off-screen because callers set `top = rect.bottom + 4` without
 * checking whether the menu actually fit below.
 *
 * Resolution policy (anchor target):
 *   1. Try below the anchor, right-aligned by default.
 *   2. If the menu would overflow the viewport bottom AND there is more
 *      room above than below, flip to above the anchor.
 *   3. Otherwise stay below and clamp the top so the menu fits.
 *   4. Horizontally: clamp into `[margin, viewport - margin]`; the
 *      menu may shift left so the right-aligned default isn't a strict
 *      invariant when the trigger is near the right edge.
 *
 * Point target (right-click): open below-right of the cursor, then
 * apply the same clamping. No flip — the user expects the menu near
 * the click.
 *
 * Measurement note: callers may invoke this on a menu that is still
 * `display:none` (via `.hidden`). We temporarily render it
 * `visibility:hidden` so `offsetWidth`/`offsetHeight` reflect the
 * actual rendered size, then leave the menu visible. The caller does
 * NOT need to toggle `.hidden` before or after.
 */

/**
 * @typedef {Object} AnchorTarget
 * @property {HTMLElement} anchor  Trigger element (e.g. the ⋯ button).
 *                                 Menu opens below it by default,
 *                                 flipping above if it doesn't fit.
 */

/**
 * @typedef {Object} PointTarget
 * @property {number} x  Page-space X (e.g. from `MouseEvent.pageX`).
 * @property {number} y  Page-space Y (e.g. from `MouseEvent.pageY`).
 */

/**
 * @typedef {Object} PositionOpts
 * @property {number}            [margin=8]   Min gap between the menu and any viewport edge.
 * @property {'right'|'left'}    [align='right']
 *   Anchor mode only — which edge of the menu lines up with the anchor.
 *   `'right'` is the typical kebab/dropdown convention.
 * @property {number}            [gap=4]      Vertical gap between menu and anchor edge.
 */

/**
 * Position a menu so it stays inside the viewport, anchored to a
 * trigger element or a click point. The menu is left visible (its
 * `.hidden` class, if present, is removed) and the caller can attach
 * dismiss handlers as usual.
 *
 * @param {HTMLElement}                menu
 * @param {AnchorTarget | PointTarget} target
 * @param {PositionOpts}               [opts]
 */
export function positionMenu(menu, target, opts = {}) {
    const margin = opts.margin ?? 8;
    const gap = opts.gap ?? 4;
    const align = opts.align ?? 'right';

    // Ensure layout so we can measure. The caller may have passed a
    // .hidden menu; render it invisibly first.
    const wasHidden = menu.classList.contains('hidden');
    let restoreVisibility = null;
    if (wasHidden) {
        restoreVisibility = menu.style.visibility;
        menu.style.visibility = 'hidden';
        menu.classList.remove('hidden');
    }

    // offsetWidth/Height fall back to a sane minimum if the menu has
    // no content yet (shouldn't happen in practice; defensive only).
    const mw = menu.offsetWidth || 200;
    const mh = menu.offsetHeight || 200;
    const vw = window.innerWidth;
    const vh = window.innerHeight;
    const sx = window.scrollX;
    const sy = window.scrollY;

    let left;
    let top;

    if ('anchor' in target) {
        const rect = target.anchor.getBoundingClientRect();
        // Horizontal: right- or left-edge alignment with the trigger,
        // converted to page space.
        left = align === 'right' ? rect.right - mw + sx : rect.left + sx;

        // Vertical: prefer below; flip above when it doesn't fit and
        // there's more room above. Both edges are still clamped below
        // — the flip is a preference, not an absolute.
        const spaceBelow = vh - rect.bottom;
        const spaceAbove = rect.top;
        const shouldFlip = mh + gap > spaceBelow && spaceAbove > spaceBelow;
        top = shouldFlip ? rect.top - mh - gap + sy : rect.bottom + gap + sy;
    } else {
        left = target.x;
        top = target.y;
    }

    // Horizontal clamp.
    const minLeft = sx + margin;
    const maxLeft = sx + vw - mw - margin;
    if (left > maxLeft) left = maxLeft;
    if (left < minLeft) left = minLeft;

    // Vertical clamp. Fixes the off-screen bug: even after the
    // anchor-flip heuristic, a very tall menu can still overflow the
    // viewport. Push it up so its bottom edge sits at `viewport -
    // margin`; if that pushes the top off the viewport, surrender and
    // clamp at the top edge (the menu is taller than the viewport).
    const minTop = sy + margin;
    const maxTop = sy + vh - mh - margin;
    if (top > maxTop) top = maxTop;
    if (top < minTop) top = minTop;

    menu.style.position = 'absolute';
    menu.style.left = `${left}px`;
    menu.style.top = `${top}px`;

    // Restore visibility (we want the menu shown, since the caller is
    // about to wire its dismiss handlers).
    if (wasHidden) {
        menu.style.visibility = restoreVisibility ?? '';
    }
}
