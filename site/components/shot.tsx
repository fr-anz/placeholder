import Image, { getImageProps, type StaticImageData } from "next/image";
import { Reveal } from "./reveal";

type Props = {
  src: StaticImageData;
  /** A tighter crop for phones, where the full window would be unreadable. */
  narrow?: StaticImageData;
  alt: string;
  priority?: boolean;
  className?: string;
  children?: React.ReactNode;
};

/** A real screenshot of the app. The files are already lossless WebP, so they
    are served as they are rather than re-encoded. */
export function Shot({ src, narrow, alt, priority, className, children }: Props) {
  const loading = priority ? ("eager" as const) : ("lazy" as const);
  let picture: React.ReactNode;
  if (narrow) {
    const { props: wide } = getImageProps({ src, alt, unoptimized: true, loading });
    const { props: small } = getImageProps({ src: narrow, alt, unoptimized: true });
    picture = (
      <picture>
        <source media="(max-width: 700px)" srcSet={small.src} width={narrow.width} height={narrow.height} />
        <img {...wide} alt={alt} fetchPriority={priority ? "high" : undefined} />
      </picture>
    );
  } else {
    picture = <Image src={src} alt={alt} unoptimized loading={loading} />;
  }
  return (
    <Reveal as="figure" variant="image" className={className ? `shot ${className}` : "shot"}>
      {children}
      {picture}
    </Reveal>
  );
}
