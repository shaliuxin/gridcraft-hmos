export const init: (config: string, callback: (id: number, message: string, data: ArrayBuffer) => void) => void;
export const input: (event: string) => void;
// Reply to a native request (file picker / save) with id, success, text and payload bytes.
export const reply: (id: number, ok: boolean, text: string, data: ArrayBuffer) => void;
// Drop a file chosen by the OS into the canvas. The native side duplicates the fd.
export const dropFile: (name: string, fd: number, x: number, y: number) => void;
// Drop raw bytes (e.g. an exported payload) into the canvas.
export const drop: (name: string, data: ArrayBuffer, x: number, y: number) => void;
// Open an in-memory document (bytes carried inline).
export const open: (name: string, data: ArrayBuffer) => void;
export const active: (active: boolean) => void;
export const close: () => void;
export const command: (command: string) => void;
