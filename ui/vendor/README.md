# Vendored xterm artifacts

These browser bundles are pinned because Buoy loads xterm as globals before the strict-TypeScript
frontend starts. Update the package, version, and SHA-256 together; renderer addons must remain
compatible with the pinned xterm core.

| File | Upstream package | Version | SHA-256 |
| --- | --- | --- | --- |
| `xterm.js` | `@xterm/xterm` | 5.5.0 | `d876256d5b64176c0d4ed4b8c0a56407dae49ff868fe8d4e1fd8f167e633441a` |
| `xterm.css` | `@xterm/xterm` | 5.5.0 | `ba8e6985669488981ccf40c0cefe3aba80722cb6c92de7ad628b0bd717faf2b6` |
| `addon-fit.js` | `@xterm/addon-fit` | 0.10.0 | `bdaefa370b1bfc42ee88d46fe6072400902a4d4b2d45cd93438dda9b23c97089` |
| `addon-canvas.js` | `@xterm/addon-canvas` | 0.7.0 | `7b3e904d5bec98b54d26674994cf994396c4af0971010ffd0d4983229b65d933` |
| `addon-search.js` | `@xterm/addon-search` | 0.15.0 | `3cf52d71d9deb4ba60125087434c53e3fb35bb2249db9b13987991fd2db1c7bd` |

All five packages are distributed by the xterm.js project under the MIT license. The search addon's
license is included in `addon-search.LICENSE`. The vendored files
are the published browser artifacts, with the local changes to `xterm.js` described below.

## Buoy underline patch

The upstream 5.5.0 `xterm.js` SHA-256 is
`1f991ac3b4b283ebf96e60ae23a00a52765dd3a2e46fa6fdda9f1aab032f7495`.
To reproduce our bundle from that artifact, apply these two exact replacements:

```js
// ExtendedAttrs: keep OSC 8 URL IDs/click handling, without a permanent dashed decoration.
// Before:
get underlineStyle(){return this._urlId?5:(469762048&this._ext)>>26}
// After:
get underlineStyle(){return(469762048&this._ext)>>26}

// InputHandler: suppress dotted/dashed SGR at the parser, for live output and replay alike.
// Before:
_processUnderline(e,t){t.extended=t.extended.clone(),(!~e||e>5)&&(e=1),
// After:
_processUnderline(e,t){t.extended=t.extended.clone(),(!~e||e>5)&&(e=1),(4===e||5===e)&&(e=0),
```

The parser retains chunk-boundary handling, colors, and single/double/curly underlines.
Native OSC 8 links still activate and underline on hover. `test/gui-terminal-repaint.ts`
checks the shipped bundle in the native webview, including fragmented SGR and OSC 8 data.
Reapply and review these changes when updating xterm; do not silently overwrite the patch.

## Buoy hidden viewport patch

In xterm 5.5.0, output in a `display:none` terminal updates the scroll area using a viewport height
of zero. This makes it one screen too short. A same-size reveal repaints the rows but does not
resynchronize the scrollbar; scrolling can no longer reach the prompt until user input triggers
`scrollToBottom`. Hidden `scrollTop` assignments can also leave the first real scroll ignored.

Apply these three replacements after the underline patch:

```js
// Public Terminal.refresh: the existing reveal/focus/wake repaint also syncs scroll geometry.
// Do not change the internal Terminal.refresh used by wheel scrolling: that would snap every
// small trackpad delta back to a whole row and prevent gradual scrolling.
// Keep buffer.ydisp intact, including when the user is reading history or viewing a search match.
// Before:
refresh(e,t){this._verifyIntegers(e,t),this._core.refresh(e,t)}
// After:
refresh(e,t){this._verifyIntegers(e,t),this._core.viewport?.syncScrollArea(),this._core.refresh(e,t)}

// Viewport.syncScrollArea: don't measure or update caches while the terminal has no layout box.
// Before:
syncScrollArea(e=!1){
// After:
syncScrollArea(e=!1){if(!this._viewportElement.offsetParent)return;

// Viewport._innerRefresh: a visible terminal may be hidden before its queued frame runs.
// Release the frame and invalidate the cached buffer length so reveal recalculates it.
// Before:
_innerRefresh(){if(this._charSizeService.height>0)
// After:
_innerRefresh(){if(!this._viewportElement.offsetParent)return this._lastRecordedBufferLength=0,void(this._refreshAnimationFrame=null);if(this._charSizeService.height>0)
```

`test/gui-terminal-scroll.ts` exercises hidden output, reading-position preservation, the first
scroll after reveal, small trackpad deltas, a hide between parsing and the queued viewport frame,
capped scrollback with resize, and normal/alternate buffer transitions. It runs against both the
Canvas and fallback DOM renderers in the native WebView. Scroll actions move the real DOM scrollbar
and never send terminal input.
Reapply and review this patch along with the underline patch when updating xterm.
