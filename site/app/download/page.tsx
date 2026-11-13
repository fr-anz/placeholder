import type { Metadata } from "next";
import { PlatformDownloads } from "@/components/platform-downloads";
import { SiteFooter } from "@/components/site-footer";

export const metadata: Metadata = {
  title: "Download Placeholder for Linux, Windows and macOS",
  description: "Download Placeholder, a native desktop app for your coding agents. Available for Linux, Windows and macOS.",
};

export default function DownloadPage() {
  return (
    <>
      <main className="download-page wrap" id="top">
        <div className="download-intro">
          <h1>Download<br />Placeholder.</h1>
        </div>
        <PlatformDownloads />
      </main>
      <SiteFooter />
    </>
  );
}
