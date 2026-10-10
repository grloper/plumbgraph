import { Client } from "@modelcontextprotocol/sdk/client/index.js";
import { StdioClientTransport } from "@modelcontextprotocol/sdk/client/stdio.js";
const [bin, root] = process.argv.slice(2);
const t = new StdioClientTransport({ command: bin, args: ["mcp", "--root", root] });
const c = new Client({ name: "sdk-check", version: "1" });
await c.connect(t);
console.log("server:", JSON.stringify(c.getServerVersion()), "caps:", JSON.stringify(c.getServerCapabilities()));
const tools = await c.listTools();
console.log("tools:", tools.tools.map(x => x.name).join(","));
for (const [name, args] of [["find_symbol",{query:"legacy_export"}],["repo_map",{tokens:400}],["dead_code",{}],["verify",{}]]) {
  try { const r = await c.callTool({ name, arguments: args }); console.log("--", name, "isError=", !!r.isError, (r.content?.[0]?.text||"").slice(0,200).replace(/\n/g," ")); }
  catch (e) { console.log("--", name, "ERROR", e.message); }
}
try { const r = await c.listResources(); console.log("resources:", JSON.stringify(r.resources.map(x=>x.uri))); 
  for (const x of r.resources.slice(0,3)) { const rd = await c.readResource({uri:x.uri}); console.log("read", x.uri, (rd.contents[0].text||"").slice(0,100).replace(/\n/g," ")); } } catch(e){ console.log("resources ERROR", e.message); }
try { const r = await c.listPrompts(); console.log("prompts", r.prompts.length);} catch(e){ console.log("prompts ERROR", e.message); }
await c.close();
