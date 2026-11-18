/* Independently written CatSystem host binding. No reference implementation code. */
(function (host, global) {
    "use strict";
    const identities = new WeakMap();
    function id(object) {
        const value = identities.get(object);
        if (value === undefined) throw new TypeError("expected a live CatSystem object");
        return value;
    }
    function construct(object, kind, args) {
        identities.set(object, host("new." + kind, args));
    }
    function call(object, name, args) {
        return host(name, [id(object)].concat(args));
    }
    function CsImage() { construct(this, "CsImage", []); }
    function CsPlane(width, height, x, y, z) {
        construct(this, "CsPlane", [
            width === undefined ? undefined : Number(width),
            height === undefined ? undefined : Number(height),
            x === undefined ? 0 : Number(x),
            y === undefined ? 0 : Number(y),
            z === undefined ? undefined : Number(z)
        ]);
    }
    function CsSound() { construct(this, "CsSound", []); }
    function geometry(prototype) {
        prototype.setPos = function (x, y, z) {
            return call(this, "object.setPos", [
                x === undefined ? undefined : Number(x),
                y === undefined ? undefined : Number(y),
                z === undefined ? undefined : Number(z)
            ]);
        };
        prototype.setDisp = function (value) { return call(this, "object.setDisp", [Number(value)]); };
        Object.defineProperty(prototype, "disp", {
            get: function () { return call(this, "object.getDisp", []); },
            set: function (value) { call(this, "object.setDisp", [Number(value)]); }
        });
        prototype.motMove = function (frames, x, y, z) {
            return call(this, "object.motMove", [
                frames === undefined ? 30 : Number(frames),
                x === undefined ? undefined : Number(x),
                y === undefined ? undefined : Number(y),
                z === undefined ? undefined : Number(z)
            ]);
        };
    }
    geometry(CsImage.prototype);
    geometry(CsPlane.prototype);
    CsImage.prototype.setPlane = function (plane) {
        return call(this, "image.setPlane", [id(plane)]);
    };
    CsImage.prototype.setBaseType = function (type) {
        return call(this, "image.setBaseType", [String(type)]);
    };
    CsImage.prototype.load = function (name) { return call(this, "image.load", [String(name)]); };
    CsSound.prototype.load = function (name, type) {
        return call(this, "sound.load", [String(name), type === undefined ? undefined : String(type)]);
    };
    CsSound.prototype.play = function (count) {
        return call(this, "sound.play", [count === undefined ? 1 : Number(count)]);
    };
    CsSound.prototype.stop = function () { return call(this, "sound.stop", []); };
    global.CsImage = CsImage;
    global.CsPlane = CsPlane;
    global.CsSound = CsSound;
    global.cfLog = function (text) { return host("cfLog", [String(text)]); };
    global.cfExit = function (value) { return host("cfExit", [value === undefined ? 0 : Number(value)]); };
    global.cfIsExecMotion = function (object) { return host("cfIsExecMotion", [id(object)]); };
    global.cfCalc = function (source) {
        if (arguments.length < 1) throw new TypeError("cfCalc expects an expression");
        if (typeof source !== "string") return 0;
        return host("cfCalc", [source]);
    };
    global.cfIsExistSave = function (slot, minimum, maximum) {
        const args = [];
        if (arguments.length > 0) args.push(Number(slot) | 0);
        if (arguments.length > 1) args.push(Number(minimum) | 0);
        if (arguments.length > 2) args.push(Number(maximum) | 0);
        return host("cfIsExistSave", args);
    };
    global.cfSaveData = function (slot) {
        return host("cfSaveData", arguments.length ? [Number(slot) | 0] : []);
    };
    global.cfUpdateSaveData = function () { return host("cfUpdateSaveData", []); };
    global.cfLoadSaveData = function (slot) {
        return host("cfLoadSaveData", arguments.length ? [Number(slot) | 0] : []);
    };
    global.cfLoadSaveDataOnlyFlag = function (slot) {
        return host("cfLoadSaveDataOnlyFlag", arguments.length ? [Number(slot) | 0] : []);
    };
    Object.defineProperty(global, "cvMouseX", {
        get: function () { return host("cvMouseX", []); },
        set: function () { throw new TypeError("cvMouseX is read-only"); }
    });
    Object.defineProperty(global, "cvMouseY", {
        get: function () { return host("cvMouseY", []); },
        set: function () { throw new TypeError("cvMouseY is read-only"); }
    });
    function variables(read, write, convert) {
        global[read] = function (index, count) {
            if (arguments.length < 1) throw new TypeError(read + " expects an index");
            index = Number(index) >>> 0;
            if (count === undefined) return host(read, [index]);
            count = Number(count) | 0;
            if (count < 1) return undefined;
            const result = [];
            for (let offset = 0; offset < count; offset++) {
                result.push(host(read, [(index + offset) >>> 0]));
            }
            return result;
        };
        global[write] = function (index, value) {
            if (arguments.length !== 2) throw new TypeError(write + " expects an index and value");
            index = Number(index) >>> 0;
            if (Array.isArray(value)) {
                for (let offset = 0; offset < value.length; offset++) {
                    host(write, [(index + offset) >>> 0, convert(value[offset])]);
                }
            } else host(write, [index, convert(value)]);
        };
    }
    variables("cfGetVar", "cfSetVar", function (value) { return Number(value) | 0; });
    variables("cfGetStrVar", "cfSetStrVar", String);
    let gameLoop = "GameLoop";
    let inputHandler = "InputHandler";
    global.cfChangeInputHandler = function (callback) {
        if (arguments.length === 0) { inputHandler = "InputHandler"; return; }
        if (typeof callback !== "string" && typeof callback !== "function") {
            throw new TypeError("cfChangeInputHandler expects a function or name");
        }
        inputHandler = callback;
    };
    global.cfChangeGameLoop = function (callback) {
        if (arguments.length === 0) { gameLoop = "GameLoop"; return; }
        if (typeof callback !== "string" && typeof callback !== "function") {
            throw new TypeError("cfChangeGameLoop expects a function or name");
        }
        gameLoop = callback;
    };
    Object.defineProperty(global, "__cs2_frame", { value: function (frame) {
        const callback = typeof gameLoop === "function" ? gameLoop : global[gameLoop];
        if (typeof callback !== "function") throw new TypeError("JavaScript GameLoop is not callable");
        return callback.call(callback, frame | 0);
    }});
    Object.defineProperty(global, "__cs2_call_named", { value: function (name, args) {
        const callback = global[name];
        if (typeof callback !== "function") throw new TypeError("JavaScript function is not callable: " + name);
        return callback.apply(callback, args);
    }});
    Object.defineProperty(global, "__cs2_input", { value: function (value, kind, x, y) {
        const callback = typeof inputHandler === "function" ? inputHandler : global[inputHandler];
        // The V8 receiver checks IsFunction and skips a missing handler.
        if (typeof callback !== "function") return undefined;
        return callback.call(callback, value, kind, x, y);
    }});
    delete global.__cs2_bridge;
})(__cs2_bridge, globalThis);
