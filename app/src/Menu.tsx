import { useEffect, useId, useRef, useState, type KeyboardEvent, type ReactNode } from "react";

export interface MenuItem {
  label: string;
  onSelect: () => void;
}

function Chevron() {
  return (
    <svg className="menu-chevron" width="8" height="8" viewBox="0 0 8 8" aria-hidden="true">
      <path d="M1.5 3 L4 5.5 L6.5 3" fill="none" stroke="currentColor" strokeWidth="1.3" strokeLinecap="round" strokeLinejoin="round" />
    </svg>
  );
}

/** A menu button: arrows move between items, Esc and Tab close it. */
export function Menu({
  items,
  label = "More",
  children,
  end = false,
}: {
  items: MenuItem[];
  /** The menu's name; also the button's when `children` draws something else. */
  label?: string;
  /** The button's face; "More" with a chevron by default. */
  children?: ReactNode;
  /** Opens leftwards, for a button at the right edge. */
  end?: boolean;
}) {
  const [open, setOpen] = useState(false);
  const id = useId();
  const button = useRef<HTMLButtonElement>(null);
  const list = useRef<HTMLDivElement>(null);
  useEffect(() => {
    if (!open) return;
    list.current?.querySelector<HTMLElement>('[role="menuitem"]')?.focus();
    const onPointer = (event: PointerEvent) => {
      const target = event.target as Node;
      if (!list.current?.contains(target) && !button.current?.contains(target)) setOpen(false);
    };
    document.addEventListener("pointerdown", onPointer);
    return () => document.removeEventListener("pointerdown", onPointer);
  }, [open]);
  const close = () => {
    setOpen(false);
    button.current?.focus();
  };
  const onKeyDown = (event: KeyboardEvent<HTMLDivElement>) => {
    const entries = [...(list.current?.querySelectorAll<HTMLElement>('[role="menuitem"]') ?? [])];
    const at = entries.indexOf(document.activeElement as HTMLElement);
    const next = { ArrowDown: at + 1, ArrowUp: at - 1, Home: 0, End: entries.length - 1 }[event.key];
    if (next !== undefined) {
      event.preventDefault();
      entries[(next + entries.length) % entries.length]?.focus();
    } else if (event.key === "Escape") {
      // The pane's own Escape would close the drawer.
      event.preventDefault();
      event.stopPropagation();
      close();
    } else if (event.key === "Tab") {
      setOpen(false);
    }
  };
  return (
    <div className={`menu${end ? " menu-end" : ""}`}>
      <button
        type="button"
        ref={button}
        className="button"
        aria-label={children ? label : undefined}
        aria-haspopup="menu"
        aria-expanded={open}
        aria-controls={open ? id : undefined}
        onClick={() => setOpen(!open)}
      >
        {children ?? (
          <>
            {label}
            <Chevron />
          </>
        )}
      </button>
      {open && (
        <div id={id} className="menu-list" role="menu" aria-label={label} ref={list} onKeyDown={onKeyDown}>
          {items.map((item) => (
            <button
              type="button"
              role="menuitem"
              tabIndex={-1}
              className="menu-item"
              key={item.label}
              onClick={() => {
                close();
                item.onSelect();
              }}
            >
              {item.label}
            </button>
          ))}
        </div>
      )}
    </div>
  );
}
