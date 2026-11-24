# SPFx fast-serve: what the official tool changes

Measured on 24 November 2026 with `spfx-fast-serve` 4.0.2, run in a scaffolded SPFx 1.21.1 gulp solution (made by our own
`spfxv.1.21.1` generator, `npm install` done first), and by reading the tool's source. The generator
`spfx-fast-serve.1.0.0` in the templates repo is built from this.

## What the tool does

1. **`gulpfile.js`.** If the text `spfx-fast-serve-helpers` is already in the file it does nothing ("patched before").
   Otherwise it replaces the match of `build\.initialize.*;` with

   ```js
   /* fast-serve */
   const { addFastServe } = require("spfx-fast-serve-helpers");
   addFastServe(build);
   /* end of fast-serve */

   build.initialize(require('gulp'));
   ```

   The initialize line comes back with single quotes whatever it had. If the file uses `build.configureWebpack.mergeConfig`
   it only warns that `fast-serve/webpack.extend.js` needs the matching changes.
2. **`package.json`.** `devDependencies["spfx-fast-serve-helpers"]` is set to `~1.<SPFx minor>.0` (`~1.21.0`), replacing a
   different value with a warning. `scripts.serve` is set to `fast-serve`, replacing an existing one with a warning. The file is
   written with the detected indentation and **without a final newline**.
3. It then asks whether to run `npm install`.

It creates **no `fast-serve/` folder**. Two optional files come from the helpers package afterwards, and are never overwritten
by a later run:

- `fast-serve webpack extend` writes `fast-serve/webpack.extend.js` (an empty `webpackConfig` and a `transformConfig` that
  returns its argument);
- `fast-serve config add` writes `fast-serve/config.json` (`{ "$schema": ".../config.v2.schema.json", "serve": {} }`).

It is for gulp solutions only (SPFx before 1.17 needs `spfx-fast-serve` 3.x).

## What the generator does differently

- It never replaces anything: an existing `serve` script or helpers version stays, and the plan says so (a developer's
  `gulp clean && gulp build && fast-serve` is a real workflow: a new `.module.scss` file only gets its `.module.scss.ts` typings
  from `gulp build`).
- The initialize line is not touched, so its quotes stay; `package.json` keeps its final newline.
- A gulpfile the official tool already patched is recognised by the same markers and left alone.
- The two optional files are choices; with the `@pnp` choice the webpack file carries the replacement of the `.module.scss`
  files that `@pnp` packages reference but do not ship (a `NormalModuleReplacementPlugin` and an empty stylesheet). Project
  aliases are never part of it.

The stock `webpack.extend.js` in the generator is the helpers package's own template (MIT, `spfx-fast-serve-helpers`).
