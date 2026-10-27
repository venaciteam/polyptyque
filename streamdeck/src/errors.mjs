export const failure = (reason, message = reason) => Object.assign(new Error(message), { reason });
