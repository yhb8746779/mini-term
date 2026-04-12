export type PtyWriteFn = (ptyId: number, data: string) => Promise<unknown>;

export function createPtyWriteQueue(writeFn: PtyWriteFn) {
  const tails = new Map<number, Promise<void>>();

  return (ptyId: number, data: string): Promise<void> => {
    const prev = tails.get(ptyId) ?? Promise.resolve();
    const run = prev.catch(() => undefined).then(async () => {
      await writeFn(ptyId, data);
    });
    const tail = run.catch(() => undefined);
    tails.set(ptyId, tail);

    return run.finally(() => {
      if (tails.get(ptyId) === tail) {
        tails.delete(ptyId);
      }
    });
  };
}
