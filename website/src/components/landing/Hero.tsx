"use client";

import { useI18n } from "@/i18n/provider";
import { useCopyToClipboard } from "@/hooks/useCopyToClipboard";
import { cn } from "@/lib/utils";
import { withBase } from "@/lib/paths";

export default function Hero() {
  const { t } = useI18n();
  const { copied, copy } = useCopyToClipboard();

  return (
    <section className="relative min-h-screen flex items-center justify-center bg-grid">
      <div className="relative z-10 mx-auto max-w-4xl px-6 pt-32 pb-24 text-center">
        {/* Badge — flat, no ping */}
        <div className="inline-flex items-center gap-2 px-3 py-1.5 rounded-md border border-border bg-surface text-xs text-text-secondary mb-10 animate-fade-in-up">
          <span className="w-1.5 h-1.5 rounded-full bg-success" />
          <span className="font-mono">{t.hero.badge}</span>
        </div>

        {/* Title — solid color, no gradient clip */}
        <h1
          className="text-5xl sm:text-6xl lg:text-7xl font-bold tracking-tight leading-[1.05] mb-6 animate-fade-in-up"
          style={{ animationDelay: "0.08s" }}
        >
          <span className="text-text-primary">{t.hero.title}</span>
          <br />
          <span className="text-accent">{t.hero.titleAccent}</span>
        </h1>

        {/* Subtitle */}
        <p
          className="max-w-xl mx-auto text-base sm:text-lg text-text-secondary leading-relaxed mb-10 animate-fade-in-up"
          style={{ animationDelay: "0.16s" }}
        >
          {t.hero.subtitle}
        </p>

        {/* CTA */}
        <div
          className="flex flex-col sm:flex-row items-center justify-center gap-3 mb-8 animate-fade-in-up"
          style={{ animationDelay: "0.24s" }}
        >
          <button
            onClick={() => copy(t.hero.installCmd)}
            className={cn(
              "group flex items-center gap-2.5 px-5 py-3 rounded-md font-mono text-sm",
              "bg-surface-raised border border-border text-text-primary",
              "hover:border-accent/40 hover:bg-surface-hover",
              "transition-colors duration-150",
              copied && "border-success/40 text-success"
            )}
          >
            <span className="text-text-tertiary select-none">$</span>
            <span className="text-left truncate max-w-[320px] sm:max-w-none">
              {copied ? "Copied" : t.hero.installCmd}
            </span>
          </button>
          <a
            href={withBase("/docs/")}
            className="px-5 py-3 rounded-md border border-border text-sm text-text-secondary hover:text-text-primary hover:bg-surface-hover transition-colors duration-150"
          >
            {t.hero.ctaSecondary}
          </a>
        </div>

        {/* Stats — no hover color change */}
        <div
          className="grid grid-cols-2 sm:grid-cols-4 gap-6 mt-16 animate-fade-in-up"
          style={{ animationDelay: "0.32s" }}
        >
          {t.hero.stats.map((stat, i) => (
            <div key={i} className="text-center">
              <div className="text-3xl sm:text-4xl font-bold font-mono text-text-primary tracking-tight">
                {stat.value}
              </div>
              <div className="text-xs text-text-tertiary mt-1.5">{stat.label}</div>
            </div>
          ))}
        </div>
      </div>
    </section>
  );
}
