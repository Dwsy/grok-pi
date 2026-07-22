import type { Metadata } from "next";
import { Geist, Geist_Mono } from "next/font/google";
import { I18nProvider } from "@/i18n/provider";
import "./globals.css";

const geistSans = Geist({
  variable: "--font-geist-sans",
  subsets: ["latin"],
});

const geistMono = Geist_Mono({
  variable: "--font-geist-mono",
  subsets: ["latin"],
});

export const metadata: Metadata = {
  title: {
    default: "grok-pi — Pi Agent Core in Grok Build's Native Terminal",
    template: "%s · grok-pi",
  },
  description:
    "Run Pi's full agent runtime inside Grok Build's production TUI. One terminal. Every model. Zero compromise.",
  openGraph: {
    title: "grok-pi",
    description: "Pi's brain. Grok's body. Your terminal.",
    type: "website",
    url: "https://dwsy.github.io/grok-pi/",
  },
  twitter: {
    card: "summary_large_image",
    title: "grok-pi",
    description: "Pi's brain. Grok's body. Your terminal.",
  },
};

export default function RootLayout({
  children,
}: Readonly<{
  children: React.ReactNode;
}>) {
  return (
    <html
      lang="en"
      className={`${geistSans.variable} ${geistMono.variable} h-full antialiased`}
      suppressHydrationWarning
    >
      <body className="min-h-full flex flex-col bg-void text-text-primary">
        <I18nProvider>{children}</I18nProvider>
      </body>
    </html>
  );
}
