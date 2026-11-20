import { describe, expect, it } from "vitest";
import {
  groupHosts,
  knownGroups,
  loadHostMeta,
  parseTags,
  saveHostMeta,
} from "./hostGroups";
import type { Profile } from "./types";

const host = (id: string, name: string): Profile => ({
  id,
  name,
  host: `${name}.example.com`,
  protocol: "ssh",
  port: 22,
  username: "ops",
  keyPath: null,
  jumpHost: null,
  color: "#9d91ec",
  favorite: false,
  tlsPinSha256: null,
  s3Region: null,
});

describe("host groups", () => {
  const profiles = [host("a", "web"), host("b", "db"), host("c", "laptop")];
  const meta = {
    a: { group: "Production", tags: ["linux", "nginx"] },
    b: { group: "Production", tags: ["linux", "postgres"] },
  };

  it("groups hosts with ungrouped ones last", () => {
    expect(
      groupHosts(profiles, meta, "").map((item) => [
        item.group,
        item.hosts.map((profile) => profile.name),
      ]),
    ).toEqual([
      ["Production", ["web", "db"]],
      ["", ["laptop"]],
    ]);
  });

  it("filters by tags and words", () => {
    expect(groupHosts(profiles, meta, "#postgres")[0].hosts[0].name).toBe("db");
    expect(
      groupHosts(profiles, meta, "#linux web")[0].hosts.map((h) => h.name),
    ).toEqual(["web"]);
    expect(groupHosts(profiles, meta, "production")[0].hosts).toHaveLength(2);
    expect(groupHosts(profiles, meta, "#windows")).toEqual([]);
  });

  it("cleans tags and stores only hosts with data", () => {
    expect(parseTags("#linux, Linux  prod,,")).toEqual(["linux", "prod"]);
    const values = new Map<string, string>();
    const storage = {
      getItem: (key: string) => values.get(key) ?? null,
      setItem: (key: string, value: string) => void values.set(key, value),
    };
    saveHostMeta(storage, { ...meta, c: { group: "", tags: [] } });
    expect(Object.keys(loadHostMeta(storage))).toEqual(["a", "b"]);
    expect(knownGroups(meta)).toEqual(["Production"]);
    values.set("weave-host-groups", "nonsense");
    expect(loadHostMeta(storage)).toEqual({});
  });
});
