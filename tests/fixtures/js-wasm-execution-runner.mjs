export function run(context) {
    context.sendTaskText(
        JSON.stringify({
            taskId: context.taskId,
            workerId: context.workerId,
            kind: context.kind,
            module: context.module,
            args: context.args.slice(),
            env: context.env.map((entry) => ({ ...entry })),
            envMap: { ...context.envMap },
            cwd: context.cwd,
            stdio: structuredCloneCompat(context.stdio),
            fds: context.fds.map((entry) => ({ ...entry })),
            ports: context.ports.map((entry) => ({ ...entry })),
            runtime: structuredCloneCompat(context.runtime),
        }),
    );
    context.sendTaskBinary(new Uint8Array([7, 11, 13, 17]));
    return Number(context.envMap.FIXTURE_EXIT_CODE ?? 1);
}

function structuredCloneCompat(value) {
    if (typeof structuredClone === "function") {
        return structuredClone(value);
    }
    return JSON.parse(JSON.stringify(value));
}
