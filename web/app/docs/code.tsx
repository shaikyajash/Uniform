// Code blocks for the docs, highlighted on the server (Shiki, "vesper"): the browser gets plain
// HTML, and only the tabs and copy buttons run there.

import { codeToHtml } from "shiki";
import { CopyCode, LangTabs, type Variant } from "./tabs";

/** Cached: the same code always gives the same HTML (and Shiki reads the clock, which Next only
 * allows outside a prerender when the result is cached). */
async function highlight(code: string, lang: string): Promise<string> {
  "use cache";
  return codeToHtml(code, { lang, theme: "vesper" });
}

/** One block: an optional title bar, the code, and a copy button. */
export async function CodeBlock({ code, lang, title }: { code: string; lang: string; title?: string }) {
  const html = await highlight(code, lang);
  return (
    <figure className="docs-code group/code relative my-5 overflow-hidden rounded-md border border-hair bg-[#0b0b0c]">
      {title && (
        <figcaption className="border-b border-hair px-4 py-2 font-mono text-[11px] tracking-[0.04em] text-faint">{title}</figcaption>
      )}
      <div className="absolute top-1.5 right-1.5">
        <CopyCode code={code} />
      </div>
      <div dangerouslySetInnerHTML={{ __html: html }} />
    </figure>
  );
}

/** The same thing in several languages, as tabs. Every tab group on the page follows one choice. */
export async function CodeTabs({ variants, title }: { variants: { key: string; label: string; lang: string; code: string }[]; title?: string }) {
  const ready: Variant[] = await Promise.all(
    variants.map(async (variant) => ({ key: variant.key, label: variant.label, code: variant.code, html: await highlight(variant.code, variant.lang) })),
  );
  return <LangTabs variants={ready} title={title} />;
}
