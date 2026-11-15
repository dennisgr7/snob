import {
  AGENTS,
  APP,
  AUTOMATIONS,
  BEHAVIOR,
  CLIENT,
  COMMAND_LINE,
  FEATURES,
  FOOTER,
  HERO,
  HERO_VIDEO,
  INSTALL,
  INSTALL_SECTION,
  ARM,
  PLATFORMS,
  SCRIPTS,
  STAR,
  STATUS,
  splitAround,
  type InstallBlock,
  type InstallMethod,
  type Point,
  type Sample,
} from '@/lib/content';
import { SITE } from '@/lib/site';
import { plainLine } from '@/lib/terminal';

/**
 * The home page as Markdown, for agents: the same words, from the same data,
 * in the same order. Lines marked as decoration on the page are left out here
 * too.
 */

const block = (lines: Sample): string =>
  ['```', ...lines.filter((line) => !('art' in line && line.art)).map(plainLine), '```'].join('\n');

const heading = (title: string, accent: string): string => `${title} ${accent}`;

export function homeToMarkdown(site: URL | string): string {
  const url = new URL('/', site).href;

  const features = FEATURES.map(
    (feature) => `### snob ${feature.name}\n\n${feature.description}\n\n${block(feature.sample)}`,
  ).join('\n\n');

  const install = INSTALL.map((method: InstallMethod) => {
    const around = splitAround(method.note, method.tool);
    const note = around && method.tool ? `${around[0]}[${method.tool.text}](${method.tool.href})${around[1]}` : method.note;
    const link = method.link ? ` [${method.link.label}](${method.link.href})` : '';
    const blocks = method.blocks
      .map((b: InstallBlock) => `${b.label ? `${b.label}:\n\n` : ''}${block(b.commands.map((command) => ({ command })))}`)
      .join('\n\n');
    return `### ${method.name}\n\n${note}${link}\n\n${blocks}`;
  }).join('\n\n');

  const platforms = PLATFORMS.map((p) => `- ${p.name}: ${p.arches.map((a) => a.name).join(', ')}`).join('\n');

  const list = (points: readonly Point[]): string => points.map((item) => `- \`${item.term}\`: ${item.means}`).join('\n');
  const status = (released: boolean): string => (released ? STATUS.released : STATUS.building);
  const tools = CLIENT.tools
    .map((tool) => `- **${tool.name}** (\`${tool.command}\`, ${status(tool.released)}): ${tool.description}`)
    .join('\n');

  return `# ${SITE.name}

> ${SITE.description}

## ${HERO.title} ${HERO.accent}

${HERO.lead}

${HERO_VIDEO.caption.replace(/^# /, '')}.

${HERO_VIDEO.description}

## ${heading(CLIENT.title, CLIENT.accent)}

${CLIENT.lead}

### ${APP.name} (${status(APP.released)})

${APP.description}

### ${CLIENT.toolsLabel}

${tools}

## ${heading(COMMAND_LINE.title, COMMAND_LINE.accent)}

${COMMAND_LINE.lead}

${features}

### ${heading(SCRIPTS.title, SCRIPTS.accent)}

${SCRIPTS.lead}

${SCRIPTS.human.twin}

${block(SCRIPTS.human.lines)}

${SCRIPTS.machine.twin}

${block(SCRIPTS.machine.lines)}

#### ${SCRIPTS.flags.title}

${list(SCRIPTS.flags.points)}

#### ${SCRIPTS.exitCodes.title}

${list(SCRIPTS.exitCodes.points)}

## ${heading(AGENTS.title, AGENTS.accent)}

${AGENTS.lead}

${list(AGENTS.points)}

${block(AGENTS.sample.lines)}

## ${heading(AUTOMATIONS.title, AUTOMATIONS.accent)}

${AUTOMATIONS.lead}

### ${AUTOMATIONS.monitor.title}

${list(AUTOMATIONS.monitor.points)}

${block(AUTOMATIONS.monitor.sample.lines)}

### ${AUTOMATIONS.webhooks.title}

${list(AUTOMATIONS.webhooks.points)}

${block(AUTOMATIONS.webhooks.sample.lines)}

## ${heading(BEHAVIOR.title, BEHAVIOR.accent)}

${BEHAVIOR.lead}

> ${BEHAVIOR.honest}

${block(BEHAVIOR.checks)}

## ${heading(INSTALL_SECTION.title, INSTALL_SECTION.accent)}

${INSTALL_SECTION.lead}

${INSTALL_SECTION.today}

${platforms}

**${ARM.label}.** ${ARM.text}

${install}

${STAR.line} [${STAR.link}](${SITE.repository})

---

Source: ${SITE.repository} · License: ${FOOTER.license} · ${FOOTER.disclaimer}

HTML version: ${url}
`;
}
