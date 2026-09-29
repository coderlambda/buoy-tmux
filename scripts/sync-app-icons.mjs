import { constants } from 'node:fs';
import { access, copyFile, mkdir, readFile, readdir } from 'node:fs/promises';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

const root = join(dirname(fileURLToPath(import.meta.url)), '..');
const source = join(root, 'assets', 'icons', 'ios');
const appleCatalog = join(
  root,
  'apps',
  'mobile',
  'src-tauri',
  'gen',
  'apple',
  'Assets.xcassets',
  'AppIcon.appiconset',
);
const check = process.argv.includes('--check');

const iconFiles = (await readdir(source))
  .filter((name) => name.endsWith('.png'))
  .sort();

await mkdir(appleCatalog, { recursive: true });
const stale = [];
for (const name of iconFiles) {
  const from = join(source, name);
  const to = join(appleCatalog, name);
  if (check) {
    try {
      await access(to, constants.R_OK);
      const [expected, actual] = await Promise.all([readFile(from), readFile(to)]);
      if (!expected.equals(actual)) stale.push(name);
    } catch {
      stale.push(name);
    }
  } else {
    await copyFile(from, to);
  }
}

if (stale.length) {
  console.error(`Mobile AppIcon is out of sync: ${stale.join(', ')}`);
  console.error('Run npm run icons:sync.');
  process.exitCode = 1;
} else {
  console.log(check ? 'Mobile AppIcon matches the shared icon set.' : 'Mobile AppIcon synchronized.');
}
