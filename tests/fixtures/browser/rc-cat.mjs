export function run(context) {
    const text = context.stdinText || context.stdio?.stdin?.value?.text || "";
    context.sendTaskText(text);
    return 0;
}
