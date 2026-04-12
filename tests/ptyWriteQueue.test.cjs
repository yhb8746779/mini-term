const assert = require('node:assert/strict');

const { createPtyWriteQueue } = require('../.tmp-tests/ptyWriteQueue.js');

function nextTick() {
  return new Promise((resolve) => setImmediate(resolve));
}

function deferred() {
  let resolve;
  let reject;
  const promise = new Promise((res, rej) => {
    resolve = res;
    reject = rej;
  });
  return { promise, resolve, reject };
}

(async () => {
  const samePtyEvents = [];
  const firstWrite = deferred();
  const enqueue = createPtyWriteQueue(async (ptyId, data) => {
    samePtyEvents.push(`start:${ptyId}:${data}`);
    if (data === 'first') {
      await firstWrite.promise;
    }
    samePtyEvents.push(`end:${ptyId}:${data}`);
  });

  const pendingFirst = enqueue(1, 'first');
  const pendingSecond = enqueue(1, 'second');
  await nextTick();

  assert.deepEqual(samePtyEvents, ['start:1:first']);

  firstWrite.resolve();
  await Promise.all([pendingFirst, pendingSecond]);

  assert.deepEqual(samePtyEvents, [
    'start:1:first',
    'end:1:first',
    'start:1:second',
    'end:1:second',
  ]);

  const parallelEvents = [];
  const blockFirstPty = deferred();
  const enqueueParallel = createPtyWriteQueue(async (ptyId, data) => {
    parallelEvents.push(`start:${ptyId}:${data}`);
    if (ptyId === 1) {
      await blockFirstPty.promise;
    }
    parallelEvents.push(`end:${ptyId}:${data}`);
  });

  const pty1 = enqueueParallel(1, 'alpha');
  const pty2 = enqueueParallel(2, 'beta');
  await nextTick();

  assert.deepEqual(parallelEvents, [
    'start:1:alpha',
    'start:2:beta',
    'end:2:beta',
  ]);

  blockFirstPty.resolve();
  await Promise.all([pty1, pty2]);
})().catch((error) => {
  console.error(error);
  process.exitCode = 1;
});
