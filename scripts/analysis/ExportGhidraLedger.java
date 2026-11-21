// Headless Ghidra post-script for exporting a compact runtime function ledger.
// The output path is supplied as the first script argument.
import ghidra.app.decompiler.DecompInterface;
import ghidra.app.decompiler.DecompileResults;
import ghidra.app.script.GhidraScript;
import ghidra.program.model.address.Address;
import ghidra.program.model.listing.Instruction;
import ghidra.program.model.listing.InstructionIterator;
import ghidra.program.model.listing.Function;
import ghidra.program.model.listing.FunctionIterator;
import ghidra.program.model.listing.Program;
import ghidra.program.model.symbol.Reference;
import ghidra.program.model.symbol.ReferenceIterator;
import ghidra.program.model.symbol.Symbol;
import ghidra.program.model.address.AddressIterator;
import ghidra.program.model.symbol.SymbolIterator;
import ghidra.util.task.ConsoleTaskMonitor;
import ghidra.util.task.TaskMonitor;
import java.io.BufferedWriter;
import java.io.File;
import java.io.FileOutputStream;
import java.io.IOException;
import java.io.OutputStreamWriter;
import java.nio.charset.StandardCharsets;
import java.util.ArrayList;
import java.util.Collections;
import java.util.HashSet;
import java.util.List;
import java.util.Set;

public class ExportGhidraLedger extends GhidraScript {
    private static final int MAX_DECOMPILED_FUNCTIONS = 8;
    private static final int DECOMPILE_TIMEOUT_SECONDS = 10;

    private static String jsonString(String value) {
        StringBuilder result = new StringBuilder("\"");
        for (int index = 0; index < value.length(); index++) {
            char character = value.charAt(index);
            switch (character) {
                case '\\': result.append("\\\\"); break;
                case '"': result.append("\\\""); break;
                case '\n': result.append("\\n"); break;
                case '\r': result.append("\\r"); break;
                case '\t': result.append("\\t"); break;
                default:
                    if (character < 0x20) {
                        result.append(String.format("\\u%04x", (int) character));
                    } else {
                        result.append(character);
                    }
            }
        }
        return result.append("\"").toString();
    }

    private static String address(Address address) {
        return address == null ? "" : address.toString();
    }

    private static List<String> imports(Program program) {
        List<String> values = new ArrayList<>();
        SymbolIterator symbols = program.getSymbolTable().getExternalSymbols();
        while (symbols.hasNext()) {
            Symbol symbol = symbols.next();
            values.add(symbol.getName() + "@" + address(symbol.getAddress()));
        }
        Collections.sort(values);
        return values;
    }

    private static List<String> callers(Function function) {
        Set<String> values = new HashSet<>();
        ReferenceIterator references = function.getProgram().getReferenceManager()
            .getReferencesTo(function.getEntryPoint());
        while (references.hasNext()) {
            Reference reference = references.next();
            if (!reference.getReferenceType().isCall()) {
                continue;
            }
            Function caller = function.getProgram().getFunctionManager()
                .getFunctionContaining(reference.getFromAddress());
            if (caller != null) {
                values.add(address(caller.getEntryPoint()));
            }
        }
        List<String> result = new ArrayList<>(values);
        Collections.sort(result);
        return result;
    }

    private static List<String> callees(Function function) {
        Set<String> values = new HashSet<>();
        InstructionIterator instructions = function.getProgram().getListing()
            .getInstructions(function.getBody(), true);
        while (instructions.hasNext()) {
            Instruction instruction = instructions.next();
            if (!instruction.getFlowType().isCall()) {
                continue;
            }
            for (Address target : instruction.getFlows()) {
                Function callee = function.getProgram().getFunctionManager()
                    .getFunctionAt(target);
                if (callee != null) {
                    values.add(address(callee.getEntryPoint()));
                }
            }
        }
        List<String> result = new ArrayList<>(values);
        Collections.sort(result);
        return result;
    }

    private static String decompile(DecompInterface decompiler, Function function,
            TaskMonitor monitor) {
        try {
            DecompileResults result = decompiler.decompileFunction(
                function, DECOMPILE_TIMEOUT_SECONDS, monitor);
            if (result != null && result.decompileCompleted()
                    && result.getDecompiledFunction() != null) {
                String text = result.getDecompiledFunction().getC();
                if (text.length() > 12000) {
                    return text.substring(0, 12000);
                }
                return text;
            }
        } catch (Exception ignored) {
            // A failed decompile remains a ledger diagnostic; the function is still exported.
        }
        return "";
    }

    @Override
    public void run() throws Exception {
        if (getScriptArgs().length < 1) {
            printerr("ExportGhidraLedger requires an output path");
            return;
        }
        Program program = currentProgram;
        if (program == null) {
            printerr("no current program");
            return;
        }
        AddressIterator entryPoints = program.getSymbolTable().getExternalEntryPointIterator();
        Address entryPoint = entryPoints.hasNext()
            ? entryPoints.next()
            : program.getImageBase();
        if (program.getFunctionManager().getFunctionAt(entryPoint) == null
                && program.getMemory().contains(entryPoint)) {
            disassemble(entryPoint);
            createFunction(entryPoint, "entrypoint");
        }
        DecompInterface decompiler = new DecompInterface();
        decompiler.openProgram(program);
        TaskMonitor monitor = new ConsoleTaskMonitor();
        List<String> external = imports(program);
        List<Function> functions = new ArrayList<>();
        FunctionIterator iterator = program.getFunctionManager().getFunctions(true);
        while (iterator.hasNext()) {
            functions.add(iterator.next());
        }
        Collections.sort(functions, (left, right) ->
            left.getEntryPoint().compareTo(right.getEntryPoint()));

        String safeName = program.getName().replaceAll("[^A-Za-z0-9._-]", "_");
        File output = new File(getScriptArgs()[0], safeName + ".json");
        try (BufferedWriter writer = new BufferedWriter(
                new OutputStreamWriter(new FileOutputStream(output), StandardCharsets.UTF_8))) {
            writer.write("{\n");
            writer.write("  \"program\": " + jsonString(program.getName()) + ",\n");
            writer.write("  \"executable_path\": "
                + jsonString(program.getExecutablePath()) + ",\n");
            writer.write("  \"language\": " + jsonString(program.getLanguageID().toString()) + ",\n");
            writer.write("  \"compiler\": " + jsonString(program.getCompilerSpec().getCompilerSpecID().toString()) + ",\n");
            writer.write("  \"imports\": [\n");
            for (int index = 0; index < external.size(); index++) {
                writer.write("    " + jsonString(external.get(index)));
                writer.write(index + 1 == external.size() ? "\n" : ",\n");
            }
            writer.write("  ],\n");
            writer.write("  \"functions\": [\n");
            int decompiledCount = 0;
            for (int index = 0; index < functions.size(); index++) {
                Function function = functions.get(index);
                List<String> callerList = callers(function);
                String functionName = function.getName();
                boolean candidate = !function.isExternal()
                    && (function.getEntryPoint().equals(entryPoint)
                        || !functionName.matches("(?i)FUN_[0-9a-f]+")
                        || callerList.isEmpty());
                String body = "";
                if (candidate && decompiledCount < MAX_DECOMPILED_FUNCTIONS) {
                    body = decompile(decompiler, function, monitor);
                    decompiledCount++;
                }
                writer.write("    {\n");
                writer.write("      \"name\": " + jsonString(function.getName()) + ",\n");
                writer.write("      \"entry\": " + jsonString(address(function.getEntryPoint())) + ",\n");
                writer.write("      \"end\": " + jsonString(address(function.getBody().getMaxAddress())) + ",\n");
                writer.write("      \"size\": " + function.getBody().getNumAddresses() + ",\n");
                writer.write("      \"callers\": [");
                for (int callerIndex = 0; callerIndex < callerList.size(); callerIndex++) {
                    writer.write(jsonString(callerList.get(callerIndex)));
                    if (callerIndex + 1 != callerList.size()) {
                        writer.write(", ");
                    }
                }
                writer.write("],\n");
                writer.write("      \"callees\": [");
                List<String> calleeList = callees(function);
                for (int calleeIndex = 0; calleeIndex < calleeList.size(); calleeIndex++) {
                    writer.write(jsonString(calleeList.get(calleeIndex)));
                    if (calleeIndex + 1 != calleeList.size()) {
                        writer.write(", ");
                    }
                }
                writer.write("],\n");
                writer.write("      \"decompile\": " + jsonString(body) + "\n");
                writer.write("    }");
                writer.write(index + 1 == functions.size() ? "\n" : ",\n");
            }
            writer.write("  ]\n");
            writer.write("}\n");
        } catch (IOException error) {
            printerr("failed to write ledger: " + error.getMessage());
        } finally {
            decompiler.dispose();
        }
    }
}
