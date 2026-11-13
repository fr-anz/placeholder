"use client";

import { useEffect, useRef, type ReactNode } from "react";

type Props = {
  as?: "div" | "figure" | "dl";
  className?: string;
  children: ReactNode;
  variant?: "heading" | "image";
  stagger?: boolean;
};

/** Only arm content below the viewport. SSR and unsupported browsers stay visible. */
export function Reveal({ as: Tag = "div", className = "", children, variant, stagger }: Props) {
  const frame = useRef<HTMLElement | null>(null);

  useEffect(() => {
    const element = frame.current;
    if (!element || !("IntersectionObserver" in window)) return;
    if (element.getBoundingClientRect().top < window.innerHeight - 48) return;

    element.dataset.reveal = "pending";
    const observer = new IntersectionObserver(
      ([entry]) => {
        if (!entry.isIntersecting) return;
        element.dataset.reveal = "visible";
        observer.disconnect();
      },
      { rootMargin: "0px 0px -48px 0px" },
    );
    observer.observe(element);
    return () => {
      observer.disconnect();
      delete element.dataset.reveal;
    };
  }, []);

  return (
    <Tag
      ref={(element: HTMLElement | null) => { frame.current = element; }}
      className={`reveal ${className}`}
      data-variant={variant}
      data-stagger={stagger || undefined}
      onFocusCapture={() => {
        // Keyboard navigation reveals the target immediately.
        if (frame.current) delete frame.current.dataset.reveal;
      }}
    >
      {children}
    </Tag>
  );
}
