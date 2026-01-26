import { Command } from 'commander';
import chalk from 'chalk';
import axios from 'axios';
import fs from 'fs';
import path from 'path';
import child_process from 'child_process';
import open from 'open';
import which from 'which';
import figlet from 'figlet';
import gradient from 'gradient-string';
import boxen from 'boxen';
import ora from 'ora';

const VERSION = '0.9'; // Updated version
const HAMMER_PATH = '/usr/lib/HackerOS/hammer/bin';
const VERSION_FILE = '/usr/lib/hammer/version.hacker';
const REMOTE_VERSION_URL = 'https://raw.githubusercontent.com/HackerOS-Linux-System/hammer/main/config/version.hacker';
const RELEASE_BASE_URL = 'https://github.com/HackerOS-Linux-System/hammer/releases/download/v';

// Color helpers with chalk
const bold = chalk.bold;
const red = chalk.red;
const green = chalk.green;
const yellow = chalk.yellow;
const blue = chalk.blue;

async function main() {
  const program = new Command();

  program
    .name('hammer')
    .version(VERSION)
    .description(gradient.pastel('Hammer CLI Tool for HackerOS Atomic'))
    .hook('preAction', () => {
      // Optional: Add a spinner or something for actions, but keep simple for now
    });

  // Install command
  program
    .command('install')
    .description('Install a package (optionally in container)')
    .argument('<package>', 'Package name')
    .option('--container', 'Install in container using distrobox')
    .action((pkg: string, options: { container?: boolean }) => {
      if (options.container) {
        runContainers('install', [pkg]);
      } else {
        runCore('install', [pkg]);
      }
    });

  // Remove command
  program
    .command('remove')
    .description('Remove a package (optionally from container)')
    .argument('<package>', 'Package name')
    .option('--container', 'Remove from container using distrobox')
    .action((pkg: string, options: { container?: boolean }) => {
      if (options.container) {
        runContainers('remove', [pkg]);
      } else {
        runCore('remove', [pkg]);
      }
    });

  // Update command
  program
    .command('update')
    .description('Update the system atomically')
    .action(() => {
      runUpdater('update', []);
    });

  // Clean command
  program
    .command('clean')
    .description('Clean up unused resources')
    .action(() => {
      runCore('clean', []);
    });

  // Refresh command
  program
    .command('refresh')
    .description('Refresh repositories')
    .action(() => {
      runCore('refresh', []);
    });

  // Build command
  program
    .command('build')
    .description('Build atomic ISO (must be in project dir)')
    .action(() => {
      runBuilder('build', []);
    });

  // Switch command
  program
    .command('switch')
    .description('Switch to a deployment (rollback if no arg)')
    .argument('[deployment]', 'Deployment name')
    .action((deployment?: string) => {
      const args = deployment ? [deployment] : [];
      runCore('switch', args);
    });

  // Deploy command
  program
    .command('deploy')
    .description('Create a new deployment')
    .action(() => {
      runCore('deploy', []);
    });

  // Build-init command
  program
    .command('build-init')
    .description('Initialize build project')
    .action(() => {
      runBuilder('init', []);
    })
    .alias('build init');

  // About command
  program
    .command('about')
    .description('Show tool information')
    .action(() => {
      about();
    });

  // TUI command
  program
    .command('tui')
    .description('Launch TUI interface')
    .action(() => {
      runTui([]);
    });

  // Status command
  program
    .command('status')
    .description('Show current deployment status')
    .action(() => {
      runCore('status', []);
    });

  // History command
  program
    .command('history')
    .description('Show deployment history')
    .action(() => {
      runCore('history', []);
    });

  // Rollback command
  program
    .command('rollback')
    .description('Rollback n steps (default 1)')
    .argument('[n]', 'Number of steps', '1')
    .action((n: string) => {
      runCore('rollback', [n]);
    });

  // Init command
  program
    .command('init')
    .description('Initialize the atomic system (linking without update)')
    .action(() => {
      runUpdater('init', []);
    });

  // Upgrade command
  program
    .command('upgrade')
    .description('Upgrade the hammer tool')
    .action(async () => {
      await upgrade();
    });

  // Issue command
  program
    .command('issue')
    .description('Open new issue in GitHub repository')
    .action(async () => {
      await issue();
    });

  program.parse(process.argv);

  if (!process.argv.slice(2).length) {
    usage(program);
  }
}

function runCore(subcommand: string, args: string[]) {
  const binary = path.join(HAMMER_PATH, 'hammer-core');
  child_process.spawnSync(binary, [subcommand, ...args], { stdio: 'inherit' });
}

function runUpdater(subcommand: string, args: string[]) {
  const binary = path.join(HAMMER_PATH, 'hammer-updater');
  child_process.spawnSync(binary, [subcommand, ...args], { stdio: 'inherit' });
}

function runBuilder(subcommand: string, args: string[]) {
  const binary = path.join(HAMMER_PATH, 'hammer-builder');
  child_process.spawnSync(binary, [subcommand, ...args], { stdio: 'inherit' });
}

function runTui(args: string[]) {
  const binary = path.join(HAMMER_PATH, 'hammer-tui');
  child_process.spawnSync(binary, args, { stdio: 'inherit' });
}

function runContainers(subcommand: string, args: string[]) {
  const binary = path.join(HAMMER_PATH, 'hammer-containers');
  child_process.spawnSync(binary, [subcommand, ...args], { stdio: 'inherit' });
}

function about() {
  figlet.text('Hammer', { font: 'Big' }, (err, data) => {
    if (err) {
      console.log(bold(blue('Hammer CLI Tool for HackerOS Atomic')));
    } else {
      console.log(gradient.retro(data || ''));
    }
    console.log(boxen(
      `${green('Version:')} ${VERSION}\n` +
      `${green('Description:')} Tool for managing atomic installations, updates, and builds inspired by apx and rpm-ostree.\n` +
      `${green('Components:')}\n` +
      `- ${yellow('hammer-core:')} Core operations in Crystal\n` +
      `- ${yellow('hammer-updater:')} System updater in Crystal\n` +
      `- ${yellow('hammer-builder:')} ISO builder in Crystal\n` +
      `- ${yellow('hammer-tui:')} TUI interface in Go with Bubble Tea\n` +
      `${green('Location:')} ${HAMMER_PATH}`,
      { padding: 1, borderStyle: 'round', borderColor: 'cyan' }
    ));
  });
}

function usage(program: Command) {
  console.log(bold(blue('Usage: hammer <command> [options]')));
  console.log('');
  console.log(green('Commands:'));
  const commands = [
    { cmd: 'install [--container] <package>', desc: 'Install a package (optionally in container)' },
    { cmd: 'remove [--container] <package>', desc: 'Remove a package (optionally from container)' },
    { cmd: 'update', desc: 'Update the system atomically' },
    { cmd: 'clean', desc: 'Clean up unused resources' },
    { cmd: 'refresh', desc: 'Refresh repositories' },
    { cmd: 'build', desc: 'Build atomic ISO (must be in project dir)' },
    { cmd: 'switch [deployment]', desc: 'Switch to a deployment (rollback if no arg)' },
    { cmd: 'deploy', desc: 'Create a new deployment' },
    { cmd: 'build init', desc: 'Initialize build project' },
    { cmd: 'about', desc: 'Show tool information' },
    { cmd: 'tui', desc: 'Launch TUI interface' },
    { cmd: 'status', desc: 'Show current deployment status' },
    { cmd: 'history', desc: 'Show deployment history' },
    { cmd: 'rollback [n]', desc: 'Rollback n steps (default 1)' },
    { cmd: 'init', desc: 'Initialize the atomic system (linking without update)' },
    { cmd: 'upgrade', desc: 'Upgrade the hammer tool' },
    { cmd: 'issue', desc: 'Open new issue in GitHub repository' },
  ];
  commands.forEach(({ cmd, desc }) => {
    console.log(` ${yellow(cmd)} ${desc}`);
  });
  // Also show default help
  program.outputHelp();
}

async function upgrade() {
  const spinner = ora('Checking for updates...').start();
  try {
    let localVersion = '0.0';
    if (fs.existsSync(VERSION_FILE)) {
      localVersion = fs.readFileSync(VERSION_FILE, 'utf8').trim().replace(/[\[\]]/g, '').trim();
    }

    const response = await axios.get(REMOTE_VERSION_URL);
    const remoteVersion = response.data.trim().replace(/[\[\]]/g, '').trim();

    if (remoteVersion > localVersion) {
      spinner.text = `Upgrading from ${localVersion} to ${remoteVersion}...`;
      const binaries = [
        { name: 'hammer', path: '/usr/bin/hammer' },
        { name: 'hammer-updater', path: path.join(HAMMER_PATH, 'hammer-updater') },
        { name: 'hammer-core', path: path.join(HAMMER_PATH, 'hammer-core') },
        { name: 'hammer-tui', path: path.join(HAMMER_PATH, 'hammer-tui') },
        { name: 'hammer-builder', path: path.join(HAMMER_PATH, 'hammer-builder') },
        { name: 'hammer-containers', path: path.join(HAMMER_PATH, 'hammer-containers') },
      ];

      for (const bin of binaries) {
        const url = `\( {RELEASE_BASE_URL} \){remoteVersion}/${bin.name}`;
        const dlSpinner = ora(`Downloading ${bin.name}...`).start();
        const res = await axios.get(url, { responseType: 'stream' });
        const writer = fs.createWriteStream(bin.path);
        res.data.pipe(writer);
        await new Promise((resolve, reject) => {
          writer.on('finish', resolve);
          writer.on('error', reject);
        });
        fs.chmodSync(bin.path, 0o755);
        dlSpinner.succeed(`Downloaded ${bin.name}`);
      }

      fs.writeFileSync(VERSION_FILE, `[ ${remoteVersion} ]`);
      spinner.succeed('Upgrade completed.');
    } else {
      spinner.warn(`Already up to date (version ${localVersion}).`);
    }
  } catch (ex) {
    spinner.fail(`Error during upgrade: ${ex.message}`);
    process.exit(1);
  }
}

async function issue() {
  const url = 'https://github.com/HackerOS-Linux-System/hammer/issues/new';
  try {
    if (which.sync('vivaldi', { nothrow: true })) {
      child_process.spawn('vivaldi', [url], { stdio: 'inherit' });
    } else if (which.sync('xdg-open', { nothrow: true })) {
      child_process.spawn('xdg-open', [url], { stdio: 'inherit' });
    } else {
      await open(url);
    }
  } catch {
    console.log(red('Error: No browser found to open the URL. Please install Vivaldi or ensure xdg-open is available.'));
    process.exit(1);
  }
}

main().catch((error) => {
  console.error(red(`Error: ${error.message}`));
  process.exit(1);
});
