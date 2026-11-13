import type { Metadata, Viewport } from "next";
import localFont from "next/font/local";
import "./globals.css";

// Archivo carries a width axis: headlines use it condensed, body at normal width.
const archivo = localFont({
  src: "./fonts/archivo-var.woff2",
  variable: "--font-archivo",
  weight: "100 900",
  display: "swap",
  declarations: [{ prop: "font-stretch", value: "62% 125%" }],
});

const jetbrainsMono = localFont({
  src: "./fonts/jetbrains-mono-var.woff2",
  variable: "--font-jetbrains-mono",
  weight: "100 800",
  display: "swap",
});

// Set NEXT_PUBLIC_SITE_URL to the deployed origin so share images resolve.
const siteUrl = process.env.NEXT_PUBLIC_SITE_URL;

export const metadata: Metadata = {
  metadataBase: siteUrl ? new URL(siteUrl) : undefined,
  title: "Placeholder: your agents, one workspace",
  description:
    "Run coding agents in parallel. Review their work in one native app.",
  openGraph: {
    title: "Placeholder",
    description: "Your agents. One workspace.",
    type: "website",
  },
};

export const viewport: Viewport = {
  themeColor: "#e7e6e2",
};

export default function RootLayout({ children }: LayoutProps<"/">) {
  return (
    <html lang="en" data-scroll-behavior="smooth" className={`${archivo.variable} ${jetbrainsMono.variable}`}>
      {/* Browser add-ons may add body attributes (for example vc-init) before
          hydration. Tolerate those here; descendants still report mismatches. */}
      <body suppressHydrationWarning>{children}</body>
    </html>
  );
}
