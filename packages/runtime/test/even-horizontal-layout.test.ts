import { describe, expect, test } from "bun:test";
import { join } from "path";

const helperPath = join(
  import.meta.dir,
  "..",
  "..",
  "..",
  "integrations",
  "tmux-plugin",
  "scripts",
  "even-horizontal-common.sh",
);

function runHelper(body: string): string {
  const result = Bun.spawnSync(
    [
      "sh",
      "-c",
      `. '${helperPath}'
${body}`,
    ],
    { stdout: "pipe", stderr: "pipe" },
  );

  if (result.exitCode !== 0) {
    throw new Error(result.stderr.toString() || `shell failed with ${result.exitCode}`);
  }

  return result.stdout.toString().trim();
}

describe("even-horizontal shell helpers", () => {
  const leftSidebarRows = [
    "%1|opensessions-sidebar|30|0|29|0",
    "%2|main-a|42|31|72|1",
    "%3|main-b|42|74|115|0",
    "%4|main-c|43|117|159|0",
  ].join("\n");

  const rightSidebarRows = [
    "%2|main-a|43|0|42|1",
    "%3|main-b|43|44|86|0",
    "%4|main-c|43|88|130|0",
    "%1|opensessions-sidebar|28|132|159|0",
  ].join("\n");

  test("counts sidebar vs non-sidebar panes", () => {
    expect(runHelper(`count_sidebar_panes '${leftSidebarRows}'`)).toBe("1");
    expect(runHelper(`count_non_sidebar_panes '${leftSidebarRows}'`)).toBe("3");
  });

  test("extracts sidebar pane metadata", () => {
    expect(runHelper(`extract_sidebar_info '${leftSidebarRows}'`)).toBe("%1|30|0|29|0");
  });

  test("detects a left sidebar from pane geometry", () => {
    expect(runHelper(`detect_sidebar_side '${leftSidebarRows}' '0' '29'`)).toBe("left");
  });

  test("detects a right sidebar from pane geometry", () => {
    expect(runHelper(`detect_sidebar_side '${rightSidebarRows}' '132' '159'`)).toBe("right");
  });

  test("returns empty for ambiguous sidebar geometry", () => {
    const ambiguousRows = [
      "%1|opensessions-sidebar|40|20|59|0",
      "%2|main-a|40|0|39|1",
      "%3|main-b|40|61|100|0",
    ].join("\n");

    expect(runHelper(`detect_sidebar_side '${ambiguousRows}' '20' '59'`)).toBe("");
  });
});
