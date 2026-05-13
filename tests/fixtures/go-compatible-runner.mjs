export default async function run(context) {
    context.sendTaskText("go-compatible-ok\n");
    return {
        exitCode: Number(context.envMap.GO_COMPAT_EXIT_CODE || 0),
        goCompatible: {
            module: context.module,
            args: context.args.slice(),
            cwd: context.cwd,
            runtime: "wasm_exec-compatible-runner",
        },
    };
}
