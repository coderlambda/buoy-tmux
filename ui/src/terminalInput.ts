// Stream each terminal's input in order. A native acknowledgement means the preceding chunk
// reached the PTY writer, so slow SSH cannot accumulate unlimited IPC calls or freeze the UI.
export const INPUT_CHUNK_UNITS = 4096;

function yieldInput(): Promise<void> {
  // WebKit throttles setTimeout to roughly one second in background windows. A posted task
  // yields to rendering/input without turning a large paste into a minutes-long transfer.
  return new Promise(resolve => {
    const channel = new MessageChannel();
    channel.port1.onmessage = () => {
      channel.port1.close(); channel.port2.close(); resolve();
    };
    channel.port2.postMessage(null);
  });
}

export function createTerminalInput(
  send: (data: string) => void | Promise<unknown>,
  canSend: () => boolean,
  onError: (error: unknown) => void,
) {
  let queue: Array<{ data: string; offset: number }> = [];
  let running = false, disposed = false, generation = 0;
  function cancel(): void { generation++; queue = []; running = false; }
  async function drain(): Promise<void> {
    if (running || disposed) return;
    running = true;
    const current = generation;
    try {
      while (queue.length && current === generation && !disposed) {
        if (!canSend()) { cancel(); return; }
        const item = queue[0]!;
        let end = Math.min(item.offset + INPUT_CHUNK_UNITS, item.data.length);
        if (end < item.data.length) {
          const last = item.data.charCodeAt(end - 1);
          // Keep surrogate pairs and CRLF intact across independently encoded native messages.
          if ((last >= 0xd800 && last <= 0xdbff) || (last === 13 && item.data.charCodeAt(end) === 10)) end--;
        }
        const chunk = item.data.slice(item.offset, end);
        await send(chunk);
        if (current !== generation || disposed) return;
        item.offset = end;
        if (end === item.data.length) queue.shift();
        // Explicitly yield during a paste, even when a bridge resolves synchronously.
        if (queue.length) await yieldInput();
      }
    } catch (error) {
      if (current === generation && !disposed) { cancel(); onError(error); }
    } finally {
      if (current === generation) running = false;
    }
  }
  return {
    push(data: string): void {
      if (!data || disposed || !canSend()) return;
      queue.push({ data, offset: 0 });
      void drain();
    },
    cancel,
    dispose(): void { disposed = true; cancel(); },
  };
}

/** Bounded recovery-command tracking. Process text runs together; slicing the draft for every
 * pasted character caused excessive copying and allocation on the webview's main thread. */
export function trackCommandText(state: { commandDraft?: string; lastCommand?: string }, data: string): void {
  const text = data.replace(/\x1b\][^\x07]*(?:\x07|\x1b\\)/g, '').replace(/\x1b\[[0-?]*[ -/]*[@-~]/g, '');
  let draft = state.commandDraft || '';
  for (const part of text.match(/[^\x00-\x1f\x7f]+|[\r\n\x7f\b\x15\x03]/gu) || []) {
    if (part === '\r' || part === '\n') {
      const command = draft.trim();
      if (command) state.lastCommand = command.slice(0, 4096);
      draft = '';
    } else if (part === '\x7f' || part === '\b') {
      const last = draft.charCodeAt(draft.length - 1);
      draft = draft.slice(0, last >= 0xdc00 && last <= 0xdfff ? -2 : -1);
    } else if (part === '\x15' || part === '\x03') {
      draft = '';
    } else {
      draft = (draft + part).slice(-4096);
    }
  }
  state.commandDraft = draft;
}
