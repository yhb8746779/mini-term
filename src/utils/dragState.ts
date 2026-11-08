export type DragPayload =
  | { type: 'project'; projectId: string }
  | { type: 'group'; groupId: string }
  | { type: 'pane'; paneId: string; sourceTabId: string };

let _payload: DragPayload | null = null;

export function setDragPayload(p: DragPayload | null) {
  _payload = p;
}

export function getDragPayload() {
  return _payload;
}
