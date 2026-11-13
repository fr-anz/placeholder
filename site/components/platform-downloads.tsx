import { BUILDS, VERSION } from "@/lib/site";
import { PlatformIcon } from "./platform-icon";
import { DownloadIcon } from "./download-icon";

export function PlatformDownloads() {
  return (
    <section className="download-options" aria-label="Choose your platform">
      {BUILDS.map(({ os, kind, arch, href, install }) => (
        <article className="platform-card" key={os}>
          <div className={`platform-mark platform-${os.toLowerCase()}`}>
            <PlatformIcon os={os} />
          </div>
          <div className="platform-heading">
            <h2>{os}</h2>
            <p>{arch}</p>
          </div>
          <p className="platform-install">{install}</p>
          <a className="btn platform-button" href={href} aria-label={`Download Placeholder ${VERSION} for ${os}, ${kind}`}>
            Download for {os}
            <DownloadIcon />
          </a>
          <p className="platform-format">{kind}</p>
        </article>
      ))}
    </section>
  );
}
