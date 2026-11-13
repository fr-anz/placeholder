import { Agents } from "@/components/agents";
import { Between } from "@/components/between";
import { Dither } from "@/components/dither";
import { Hero } from "@/components/hero";
import { Inside } from "@/components/inside";
import { RunStory } from "@/components/run-story";
import { SiteFooter } from "@/components/site-footer";

export default function Home() {
  return (
    <>
      <main id="top">
        <Hero />
        <Agents />
        <RunStory />
        <Between />
        <Inside />
        <Dither />
      </main>
      <SiteFooter />
    </>
  );
}
