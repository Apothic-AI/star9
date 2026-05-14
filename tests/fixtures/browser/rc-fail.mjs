export function run(context) {
    context.sendTaskText("browser-fail\n");
    return Number(context.args[0] || 1);
}
