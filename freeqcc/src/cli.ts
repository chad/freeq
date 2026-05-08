#!/usr/bin/env node
// Phase 7 — commander-driven CLI: launch | status | stop | doctor.
// Skeleton in place; real wiring happens in phase 7.
import { Command } from "commander";

const program = new Command();
program
  .name("freeqcc")
  .description("freeq + Claude Code: launch a freeq-DM-controllable Claude Code agent.")
  .version("0.1.0");

program
  .command("launch")
  .description("Launch the freeqcc agent (first run prompts for handle + bot nick).")
  .action(async () => {
    console.log("freeqcc launch — not implemented yet (phase 7)");
    process.exitCode = 1;
  });

program
  .command("status")
  .description("Show daemon status: connected, owner verified, bot DID/nick, last DM.")
  .action(async () => {
    console.log("freeqcc status — not implemented yet (phase 7)");
    process.exitCode = 1;
  });

program
  .command("stop")
  .description("Stop the running daemon (clean QUIT).")
  .action(async () => {
    console.log("freeqcc stop — not implemented yet (phase 7)");
    process.exitCode = 1;
  });

program
  .command("doctor")
  .description("Sanity-check config, identity, delegation, owner resolution.")
  .action(async () => {
    console.log("freeqcc doctor — not implemented yet (phase 7)");
    process.exitCode = 1;
  });

program.parseAsync(process.argv).catch((err) => {
  console.error(err);
  process.exit(1);
});
