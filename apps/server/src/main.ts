import {
  AmpAgentWatcher,
  ClaudeCodeAgentWatcher,
  CodexAgentWatcher,
  OpenCodeAgentWatcher,
  PiAgentWatcher,
  PluginLoader,
  SERVER_HOST,
  SERVER_PORT,
  loadConfig,
  startServer,
} from "@opensessions/runtime";
import { join } from "path";

const config = loadConfig();
const loader = new PluginLoader();

for (const pkg of ["@opensessions/mux-tmux", "@opensessions/mux-zellij"]) {
  try {
    const mod = require(pkg);
    const factory = typeof mod.default === "function" ? mod.default : mod;
    factory({
      registerMux: (provider: any) => loader.registerMux(provider),
      serverPort: SERVER_PORT,
      serverHost: SERVER_HOST,
    });
  } catch {
    // Plugin not installed — skip
  }
}

const home = process.env.HOME ?? process.env.USERPROFILE ?? "";
const pluginDir = join(home, ".config", "opensessions", "plugins");
const localPlugins = loader.loadDir(pluginDir);
if (localPlugins.length > 0) {
  console.log(`Loaded local plugins: ${localPlugins.join(", ")}`);
}

if (config.plugins.length > 0) {
  const npmPlugins = loader.loadPackages(config.plugins);
  if (npmPlugins.length > 0) {
    console.log(`Loaded npm plugins: ${npmPlugins.join(", ")}`);
  }
}

const mux = loader.resolve(config.mux);
if (!mux) {
  console.error(
    "No terminal multiplexer detected.\n" +
    `Registered providers: ${loader.registry.list().join(", ") || "(none)"}\n` +
    "Are you running inside tmux or zellij?\n" +
    "Set 'mux' in ~/.config/opensessions/config.json to override.",
  );
  process.exit(1);
}

const extraProviders = loader.registry.list()
  .filter((name) => name !== mux.name)
  .map((name) => loader.registry.get(name)!)
  .filter(Boolean);

if (extraProviders.length > 0) {
  console.log(`Extra mux providers: ${extraProviders.map((provider) => provider.name).join(", ")}`);
}

loader.registerWatcher(new AmpAgentWatcher());
loader.registerWatcher(new ClaudeCodeAgentWatcher());
loader.registerWatcher(new CodexAgentWatcher());
loader.registerWatcher(new OpenCodeAgentWatcher());
loader.registerWatcher(new PiAgentWatcher());

const watchers = loader.getWatchers();
if (watchers.length > 0) {
  console.log(`Agent watchers: ${watchers.map((watcher) => watcher.name).join(", ")}`);
}

console.log(`Primary mux provider: ${mux.name}`);
startServer(mux, extraProviders, watchers);
