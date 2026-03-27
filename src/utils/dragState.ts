let _tabId: string | null = null;

export function setDraggingTabId(id: string | null) {
  _tabId = id;
}

export function getDraggingTabId() {
  return _tabId;
}
