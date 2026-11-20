// export-hugeicon-svgs.mjs — materialise the exact HugeIcons from the installed
// package as standalone SVG files, one per semantic icon in pixel-icon-map.json.
// Usage: node tools/export-hugeicon-svgs.mjs <outDir>
// Run from anywhere; resolves the package and the map relative to this file.
import { createRequire } from 'node:module';
import { mkdirSync, readFileSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';

const outDir = process.argv[2];
if (!outDir) { console.error('usage: node tools/export-hugeicon-svgs.mjs <outDir>'); process.exit(1); }
const require = createRequire(new URL('../ui/package.json', import.meta.url));
const H = require('@hugeicons/core-free-icons');
const map = JSON.parse(readFileSync(new URL('./pixel-icon-map.json', import.meta.url), 'utf8'));
mkdirSync(outDir, { recursive: true });

let count = 0;
for (const [semantic, exportName] of Object.entries(map)) {
  if (semantic.startsWith('_')) continue;
  const icon = H[exportName];
  if (!icon || !Array.isArray(icon)) throw new Error(`HugeIcons export not found: ${exportName} (for ${semantic})`);
  const shapes = icon.map(([tag, attrs]) => {
    const a = Object.entries(attrs)
      .map(([k, v]) => `${k}="${String(v).replace(/"/g, '&quot;')}"`)
      .join(' ');
    return `<${tag} ${a}/>`;
  }).join('\n  ');
  const svg = `<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" width="24" height="24" fill="none" stroke="#000000" stroke-linecap="round" stroke-linejoin="round">\n  ${shapes}\n</svg>\n`;
  writeFileSync(join(outDir, `${semantic}.svg`), svg);
  count++;
}
console.log(`exported ${count} HugeIcons SVGs to ${outDir}`);
