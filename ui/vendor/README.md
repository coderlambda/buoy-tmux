# Vendored xterm artifacts

These browser bundles are pinned because Buoy loads xterm as globals before the strict-TypeScript
frontend starts. Update the package, version, and SHA-256 together; renderer addons must remain
compatible with the pinned xterm core.

| File | Upstream package | Version | SHA-256 |
| --- | --- | --- | --- |
| `xterm.js` | `@xterm/xterm` | 5.5.0 | `5434deb0fe2acb7478466a78af7c41d0efd752cd7e376efb8fdada291c9a87c6` |
| `xterm.css` | `@xterm/xterm` | 5.5.0 | `ba8e6985669488981ccf40c0cefe3aba80722cb6c92de7ad628b0bd717faf2b6` |
| `addon-fit.js` | `@xterm/addon-fit` | 0.10.0 | `bdaefa370b1bfc42ee88d46fe6072400902a4d4b2d45cd93438dda9b23c97089` |
| `addon-canvas.js` | `@xterm/addon-canvas` | 0.7.0 | `7b3e904d5bec98b54d26674994cf994396c4af0971010ffd0d4983229b65d933` |

All four packages are distributed by the xterm.js project under the MIT license. The vendored files
are the published browser artifacts, with two local changes to `xterm.js` described below.

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
