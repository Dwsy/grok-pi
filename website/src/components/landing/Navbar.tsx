"use client";

import { useState, useEffect } from "react";
import { useI18n } from "@/i18n/provider";
import { locales, type Locale } from "@/i18n/config";
import { cn } from "@/lib/utils";
import { withBase } from "@/lib/paths";

const links = [
  { href: "#features", key: "features" },
  { href: "#comparison", key: "comparison" },
  { href: "#architecture", key: "architecture" },
  { href: "#migration", key: "migration" },
] as const;

export default function Navbar() {
  const { t, locale, setLocale } = useI18n();
  const [scrolled, setScrolled] = useState(false);
  const [mobileOpen, setMobileOpen] = useState(false);

  useEffect(() => {
    const onScroll = () => setScrolled(window.scrollY > 24);
    window.addEventListener("scroll", onScroll, { passive: true });
    return () => window.removeEventListener("scroll", onScroll);
  }, []);

  return (
    <header
      className={cn(
        "fixed top-0 left-0 right-0 z-50 transition-colors duration-150",
        scrolled
          ? "bg-void/95 border-b border-border"
          : "bg-transparent border-b border-transparent"
      )}
    >
      <nav className="mx-auto max-w-6xl px-6 h-14 flex items-center justify-between">
        {/* Logo */}
        <a href="#" className="flex items-center gap-2">
          <span className="font-mono text-base font-bold text-accent">π</span>
          <span className="text-sm font-medium text-text-primary">grok-pi</span>
        </a>

        {/* Desktop links */}
        <div className="hidden md:flex items-center gap-0.5">
          {links.map((link) => (
            <a
              key={link.href}
              href={link.href}
              className="px-3 py-1.5 text-[13px] text-text-secondary hover:text-text-primary rounded-md hover:bg-surface-hover transition-colors duration-150"
            >
              {t.nav[link.key]}
            </a>
          ))}
          <a
            href={withBase("/docs/")}
            className="px-3 py-1.5 text-[13px] text-text-secondary hover:text-text-primary rounded-md hover:bg-surface-hover transition-colors duration-150"
          >
            {t.nav.docs}
          </a>
        </div>

        {/* Right side */}
        <div className="flex items-center gap-2.5">
          {/* Language switcher */}
          <div className="flex items-center rounded-md border border-border overflow-hidden">
            {locales.map((l: Locale) => (
              <button
                key={l}
                onClick={() => setLocale(l)}
                className={cn(
                  "px-2 py-1 text-[11px] font-medium transition-colors duration-150",
                  locale === l
                    ? "bg-surface-hover text-text-primary"
                    : "text-text-tertiary hover:text-text-secondary"
                )}
              >
                {l.toUpperCase()}
              </button>
            ))}
          </div>

          {/* GitHub */}
          <a
            href="https://github.com/Dwsy/grok-pi"
            target="_blank"
            rel="noopener noreferrer"
            className="hidden sm:flex items-center justify-center w-8 h-8 rounded-md text-text-secondary hover:text-text-primary hover:bg-surface-hover transition-colors duration-150"
            aria-label="GitHub"
          >
            <svg className="w-4 h-4" fill="currentColor" viewBox="0 0 24 24">
              <path d="M12 0c-6.626 0-12 5.373-12 12 0 5.302 3.438 9.8 8.207 11.387.599.111.793-.261.793-.577v-2.234c-3.338.726-4.033-1.416-4.033-1.416-.546-1.387-1.333-1.756-1.333-1.756-1.089-.745.083-.729.083-.729 1.205.084 1.839 1.237 1.839 1.237 1.07 1.834 2.807 1.304 3.492.997.107-.775.418-1.305.762-1.604-2.665-.305-5.467-1.334-5.467-5.931 0-1.311.469-2.381 1.236-3.221-.124-.303-.535-1.524.117-3.176 0 0 1.008-.322 3.301 1.23.957-.266 1.983-.399 3.003-.404 1.02.005 2.047.138 3.006.404 2.291-1.552 3.297-1.23 3.297-1.23.653 1.653.242 2.874.118 3.176.77.84 1.235 1.911 1.235 3.221 0 4.609-2.807 5.624-5.479 5.921.43.372.823 1.102.823 2.222v3.293c0 .319.192.694.801.576 4.765-1.589 8.199-6.086 8.199-11.386 0-6.627-5.373-12-12-12z" />
            </svg>
          </a>

          {/* Download CTA */}
          <a
            href="#download"
            className="hidden sm:inline-flex items-center px-3.5 py-1.5 rounded-md border border-border text-[13px] font-medium text-text-primary hover:bg-surface-hover transition-colors duration-150"
          >
            {t.nav.download}
          </a>

          {/* Mobile menu */}
          <button
            onClick={() => setMobileOpen(!mobileOpen)}
            className="md:hidden flex items-center justify-center w-8 h-8 rounded-md text-text-secondary hover:text-text-primary transition-colors duration-150"
          >
            <svg className="w-5 h-5" fill="none" stroke="currentColor" viewBox="0 0 24 24">
              {mobileOpen ? (
                <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={1.5} d="M6 18L18 6M6 6l12 12" />
              ) : (
                <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={1.5} d="M4 7h16M4 12h16M4 17h16" />
              )}
            </svg>
          </button>
        </div>
      </nav>

      {/* Mobile dropdown — solid bg, no glass */}
      {mobileOpen && (
        <div className="md:hidden bg-void border-t border-border px-6 py-3 space-y-0.5">
          {links.map((link) => (
            <a
              key={link.href}
              href={link.href}
              onClick={() => setMobileOpen(false)}
              className="block px-3 py-2 text-sm text-text-secondary hover:text-text-primary rounded-md hover:bg-surface-hover transition-colors duration-150"
            >
              {t.nav[link.key]}
            </a>
          ))}
          <a
            href={withBase("/docs/")}
            onClick={() => setMobileOpen(false)}
            className="block px-3 py-2 text-sm text-text-secondary hover:text-text-primary rounded-md hover:bg-surface-hover transition-colors duration-150"
          >
            {t.nav.docs}
          </a>
          <a
            href="#download"
            onClick={() => setMobileOpen(false)}
            className="block px-3 py-2 text-sm text-accent font-medium"
          >
            {t.nav.download}
          </a>
        </div>
      )}
    </header>
  );
}
