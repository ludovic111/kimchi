// Pointer-driven drag of library assets onto the timeline or the composer.
// (HTML5 drag-and-drop is unreliable inside webviews that also accept file drops.)

export interface DropTarget {
  kind: "track";
  trackId: string;
  time: number;
}

class DragState {
  assetId = $state<string | null>(null);
  x = $state(0);
  y = $state(0);
  /** Set by whoever is under the pointer. */
  target = $state<DropTarget | null>(null);
  /** Set when hovering the composer's reference slots. */
  overComposer = $state(false);

  get active() {
    return this.assetId !== null;
  }

  start(assetId: string, e: PointerEvent) {
    this.assetId = assetId;
    this.x = e.clientX;
    this.y = e.clientY;
    this.target = null;
    this.overComposer = false;
  }

  end() {
    this.assetId = null;
    this.target = null;
    this.overComposer = false;
  }
}

export const drag = new DragState();
