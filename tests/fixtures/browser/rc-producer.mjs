export function run(context) {
    const text = context.args[0] || "browser-pipe-ok\n";
    context.sendTaskText(text.endsWith("\n") ? text : `${text}\n`);
    return 0;
}
