/** Writes `protocol/schema.json`: the JSON Schema of every RPC method and notification, for non-TS clients. */
import { join } from "node:path";

import { Methods, Notifications, Origin, PROTOCOL_VERSION } from "../src/protocol.ts";

const out = join(import.meta.dir, "../../protocol/schema.json");
const schema = { protocol: PROTOCOL_VERSION, definitions: { Origin }, methods: Methods, notifications: Notifications };
await Bun.write(out, `${JSON.stringify(schema, null, 2)}\n`);
console.log(`wrote ${out}`);
