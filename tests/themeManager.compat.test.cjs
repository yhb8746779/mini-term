const assert = require('node:assert/strict');

const dataset = {};
const storage = new Map();
let mediaQueryListener;

global.document = {
  documentElement: { dataset },
};

global.localStorage = {
  getItem(key) {
    return storage.get(key) ?? null;
  },
  setItem(key, value) {
    storage.set(key, value);
  },
};

global.window = {
  matchMedia() {
    return {
      matches: false,
      addListener(listener) {
        mediaQueryListener = listener;
      },
      removeListener(listener) {
        if (mediaQueryListener === listener) {
          mediaQueryListener = undefined;
        }
      },
    };
  },
  dispatchEvent() {
    return true;
  },
};

const { applyTheme, getResolvedTheme } = require('../.tmp-tests/themeManager.js');

assert.doesNotThrow(() => applyTheme('auto'));
assert.equal(getResolvedTheme(), 'dark');
assert.equal(dataset.theme, 'dark');
assert.equal(storage.get('mini-term-theme'), 'dark');
assert.equal(typeof mediaQueryListener, 'function');
