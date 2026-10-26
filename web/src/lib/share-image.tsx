import { readFile } from "node:fs/promises";
import { join } from "node:path";
import { ImageResponse } from "next/og";

export const alt = "LetsWritePaper, an editor that keeps the author of every edit";
export const size = { width: 1200, height: 630 };
export const contentType = "image/png";

export async function shareImage() {
  const icon = await readFile(join(process.cwd(), "public/icon.png"));
  const src = `data:image/png;base64,${icon.toString("base64")}`;

  return new ImageResponse(
    (
      <div
        style={{
          width: "100%",
          height: "100%",
          display: "flex",
          flexDirection: "column",
          justifyContent: "space-between",
          background: "#f3f0e8",
          color: "#1c1915",
          padding: "72px",
        }}
      >
        <div style={{ display: "flex", alignItems: "center", gap: 18 }}>
          {/* eslint-disable-next-line @next/next/no-img-element */}
          <img src={src} alt="" width={72} height={72} />
          <div style={{ display: "flex", fontSize: 32, letterSpacing: "-0.03em" }}>LetsWritePaper</div>
        </div>
        <div style={{ display: "flex", fontSize: 76, lineHeight: 1.02, letterSpacing: "-0.04em", maxWidth: 980 }}>
          Every edit keeps its author.
        </div>
        <div style={{ display: "flex", fontSize: 30, color: "#5f584e" }}>
          Version control and AI co-authors for LaTeX and Typst.
        </div>
      </div>
    ),
    { ...size },
  );
}
