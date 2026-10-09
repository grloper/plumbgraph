import { used } from './lib.js';
import fs from 'node:fs';

function deadLocal() {
  return 1;
}

export function exportedUnused() {
  return 2;
}

console.log(used(), fs.existsSync('.'));

export function htmlOnly() {
  return 3;
}
