/**
 * Local pointer state shared between the input handler and the cursor overlay. Plain module state plus an
 * event target: mouse movement at 100+ Hz must never go through React state.
 */
export const localPointer = { x: 0, y: 0, at: 0, inside: false };
export const pointerBus = new EventTarget();

export function updateLocalPointer(x: number, y: number, inside = true): void {
  localPointer.x = x;
  localPointer.y = y;
  localPointer.at = performance.now();
  localPointer.inside = inside;
  pointerBus.dispatchEvent(new Event("move"));
}
