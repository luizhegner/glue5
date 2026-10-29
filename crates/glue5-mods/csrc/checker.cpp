// The Luau type checker for GLUE 5 Studio: Luau.Analysis over files kept in
// memory, with the natives' definitions, behind a C API that answers in JSON
// (checker.rs). Not thread-safe: the caller serialises the calls.

#include "Luau/AstQuery.h"
#include "Luau/Autocomplete.h"
#include "Luau/BuiltinDefinitions.h"
#include "Luau/Frontend.h"
#include "Luau/ToString.h"

#include <cstdlib>
#include <cstring>
#include <memory>
#include <string>
#include <unordered_map>
#include <vector>

namespace {

std::string quote(std::string_view s) {
  std::string out = "\"";
  for (unsigned char c : s) {
    switch (c) {
      case '"': out += "\\\""; break;
      case '\\': out += "\\\\"; break;
      case '\n': out += "\\n"; break;
      case '\r': out += "\\r"; break;
      case '\t': out += "\\t"; break;
      default:
        if (c < 0x20) {
          char buf[8];
          snprintf(buf, sizeof buf, "\\u%04x", c);
          out += buf;
        } else {
          out += char(c);
        }
    }
  }
  return out + "\"";
}

std::string range(const Luau::Location& l) {
  return "[" + std::to_string(l.begin.line) + "," + std::to_string(l.begin.column) + "," + std::to_string(l.end.line) +
         "," + std::to_string(l.end.column) + "]";
}

char* owned(const std::string& s) {
  char* p = static_cast<char*>(malloc(s.size() + 1));
  if (!p) abort();
  memcpy(p, s.c_str(), s.size() + 1);
  return p;
}

// Files by module name, `<mod>/<file>.luau`; `require("./menu")` from a
// file of a mod is `menu.luau` beside it, `../menu` the folder above's (as
// the runtime's require, vm.rs), never out of the mod's folder.
struct Files : Luau::FileResolver {
  std::unordered_map<std::string, std::string> sources;

  std::optional<Luau::SourceCode> readSource(const Luau::ModuleName& name) override {
    auto it = sources.find(name);
    if (it == sources.end()) return std::nullopt;
    return Luau::SourceCode{it->second, Luau::SourceCode::Module};
  }

  std::optional<Luau::ModuleInfo> resolveModule(const Luau::ModuleInfo* context, Luau::AstExpr* expr,
                                                const Luau::TypeCheckLimits&) override {
    auto* str = expr->as<Luau::AstExprConstantString>();
    if (!str || !context) return std::nullopt;
    std::string path(str->value.data, str->value.size);
    // The folders: the mod's, then down to the file that asks.
    std::vector<std::string> at;
    size_t start = 0;
    for (size_t slash; (slash = context->name.find('/', start)) != std::string::npos; start = slash + 1)
      at.push_back(context->name.substr(start, slash - start));
    if (at.empty()) return std::nullopt;
    std::vector<std::string> parts;
    start = 0;
    for (size_t slash;; start = slash + 1) {
      slash = path.find('/', start);
      parts.push_back(path.substr(start, slash == std::string::npos ? std::string::npos : slash - start));
      if (slash == std::string::npos) break;
    }
    if (parts[0] != "." && parts[0] != "..") return std::nullopt;
    for (size_t i = 0; i < parts.size(); i++) {
      const std::string& p = parts[i];
      if (p == "." && i == 0) continue;
      if (p == "..") {
        // The mod's own folder is the last one to go up to.
        if (at.size() <= 1) return std::nullopt;
        at.pop_back();
      } else if (p.empty() || p[0] == '.') {
        return std::nullopt;
      } else {
        at.push_back(p);
      }
    }
    std::string module;
    for (size_t i = 0; i < at.size(); i++) module += (i ? "/" : "") + at[i];
    return Luau::ModuleInfo{module + ".luau"};
  }
};

// Nonstrict unless a file asks for `--!strict`.
struct Configs : Luau::ConfigResolver {
  Luau::Config config;
  Configs() { config.mode = Luau::Mode::Nonstrict; }
  const Luau::Config& getConfig(const Luau::ModuleName&, const Luau::TypeCheckLimits&) const override { return config; }
};

Luau::FrontendOptions options() {
  Luau::FrontendOptions o;
  o.retainFullTypeGraphs = true;
  o.runLintChecks = true;
  return o;
}

}  // namespace

struct Glue5Checker {
  Files files;
  Configs configs;
  Luau::Frontend frontend;
  Glue5Checker() : frontend(&files, &configs, options()) {}
};

extern "C" {

// A checker with Luau's globals and `definitions` (a definition file);
// null with `*error` set when they do not load.
Glue5Checker* glue5_checker_new(const char* definitions, size_t len, char** error) {
  try {
    // Freed if loading the definitions throws.
    std::unique_ptr<Glue5Checker> c(new Glue5Checker());
    std::string_view source(definitions, len);
    Luau::registerBuiltinGlobals(c->frontend, c->frontend.globals, false);
    Luau::registerBuiltinGlobals(c->frontend, c->frontend.globalsForAutocomplete, true);
    auto loaded = c->frontend.loadDefinitionFile(c->frontend.globals, c->frontend.globals.globalScope, source, "@glue5",
                                                 false, false);
    auto loadedForAutocomplete = c->frontend.loadDefinitionFile(
        c->frontend.globalsForAutocomplete, c->frontend.globalsForAutocomplete.globalScope, source, "@glue5", false, true);
    if (!loaded.success || !loadedForAutocomplete.success) {
      std::string message = "the definitions do not load:";
      for (const auto& e : loaded.parseResult.errors) {
        message += " " + std::to_string(e.getLocation().begin.line + 1) + ": " + e.getMessage();
      }
      if (loaded.module) {
        for (const auto& e : loaded.module->errors) {
          message += " " + std::to_string(e.location.begin.line + 1) + ": " + Luau::toString(e);
        }
      }
      *error = owned(message);
      return nullptr;
    }
    Luau::freeze(c->frontend.globals.globalTypes);
    Luau::freeze(c->frontend.globalsForAutocomplete.globalTypes);
    return c.release();
  } catch (const std::exception& e) {
    *error = owned(e.what());
    return nullptr;
  }
}

void glue5_checker_free(Glue5Checker* c) { delete c; }

void glue5_string_free(char* s) { free(s); }

// Sets (or, with a null source, removes) a file.
void glue5_checker_set(Glue5Checker* c, const char* name, const char* source, size_t len) {
  // No exception into Rust (it would abort the game): out of memory, the
  // file is left as it was.
  try {
    if (source) {
      c->files.sources[name] = std::string(source, len);
    } else {
      c->files.sources.erase(name);
    }
    c->frontend.markDirty(name);
  } catch (...) {
  }
}

// [{"range":[l0,c0,l1,c1],"severity":"error"|"warning","code":"...","message":"..."}]
char* glue5_checker_check(Glue5Checker* c, const char* name) {
  try {
    Luau::CheckResult result = c->frontend.check(name);
    std::string out = "[";
    auto add = [&](const Luau::Location& l, const char* severity, std::string_view code, const std::string& message) {
      if (out.size() > 1) out += ",";
      out += "{\"range\":" + range(l) + ",\"severity\":\"" + severity + "\",\"code\":" + quote(code) +
             ",\"message\":" + quote(message) + "}";
    };
    for (const auto& e : result.errors) {
      if (e.moduleName != name) continue;
      add(e.location, "error", Luau::get<Luau::SyntaxError>(e) ? "syntax" : "type",
          Luau::toString(e, Luau::TypeErrorToStringOptions{c->frontend.fileResolver}));
    }
    for (const auto& w : result.lintResult.errors) {
      add(w.location, "error", Luau::LintWarning::getName(w.code), w.text);
    }
    for (const auto& w : result.lintResult.warnings) {
      add(w.location, "warning", Luau::LintWarning::getName(w.code), w.text);
    }
    return owned(out + "]");
  } catch (const std::exception& e) {
    return owned(std::string("{\"error\":") + quote(e.what()) + "}");
  }
}

// [{"label":"...","kind":n,"detail":"...","doc":"@glue5/global/...","deprecated":bool,"correct":bool,"call":bool}]
// Types in full: the host writes the catalog's long unions of names as
// their aliases (VehicleModel...), which a truncated union would not match.
static std::string typeText(const std::string& label, Luau::TypeId type) {
  Luau::ToStringOptions opts;
  opts.maxTypeLength = 0;
  if (auto* f = Luau::get<Luau::FunctionType>(Luau::follow(type)); f && !label.empty()) {
    return Luau::toStringNamedFunction(label, *f, opts);
  }
  return Luau::toString(type, opts);
}

char* glue5_checker_complete(Glue5Checker* c, const char* name, unsigned line, unsigned column) {
  try {
    Luau::FrontendOptions o = options();
    o.forAutocomplete = true;
    c->frontend.check(name, o);
    Luau::AutocompleteResult result = Luau::autocomplete(c->frontend, name, Luau::Position{line, column}, {});
    std::string out = "[";
    for (const auto& [label, entry] : result.entryMap) {
      std::string detail;
      if (entry.type) {
        detail = typeText(Luau::get<Luau::FunctionType>(Luau::follow(*entry.type)) ? label : "", *entry.type);
      }
      if (out.size() > 1) out += ",";
      out += "{\"label\":" + quote(label) + ",\"kind\":" + std::to_string(int(entry.kind)) + ",\"detail\":" + quote(detail) +
             ",\"doc\":" + quote(entry.documentationSymbol.value_or("")) +
             ",\"deprecated\":" + (entry.deprecated ? "true" : "false") +
             ",\"correct\":" + (entry.typeCorrect != Luau::TypeCorrectKind::None ? "true" : "false") +
             ",\"call\":" + (entry.parens != Luau::ParenthesesRecommendation::None ? "true" : "false") + "}";
    }
    return owned(out + "]");
  } catch (const std::exception& e) {
    return owned(std::string("{\"error\":") + quote(e.what()) + "}");
  }
}

// {"type":"...","doc":"@glue5/global/...","range":[...]} or null.
char* glue5_checker_hover(Glue5Checker* c, const char* name, unsigned line, unsigned column) {
  try {
    c->frontend.check(name);
    const Luau::SourceModule* source = c->frontend.getSourceModule(name);
    Luau::ModulePtr module = c->frontend.moduleResolver.getModule(name);
    if (!source || !module) return owned("null");
    Luau::Position at{line, column};
    Luau::AstExpr* expr = Luau::findExprAtPosition(*source, at);
    auto type = Luau::findTypeAtPosition(*module, *source, at);
    if (!expr || !type) return owned("null");
    std::string text;
    std::string label;
    if (auto* index = expr->as<Luau::AstExprIndexName>()) label = index->index.value;
    if (auto* global = expr->as<Luau::AstExprGlobal>()) label = global->name.value;
    if (auto* local = expr->as<Luau::AstExprLocal>()) label = local->local->name.value;
    if (Luau::get<Luau::FunctionType>(Luau::follow(*type)) && !label.empty()) {
      text = typeText(label, *type);
    } else {
      text = (label.empty() ? "" : label + ": ") + typeText("", *type);
    }
    auto doc = Luau::getDocumentationSymbolAtPosition(*source, *module, at);
    return owned("{\"type\":" + quote(text) + ",\"doc\":" + quote(doc.value_or("")) + ",\"range\":" +
                 range(expr->location) + "}");
  } catch (const std::exception& e) {
    return owned(std::string("{\"error\":") + quote(e.what()) + "}");
  }
}

// The call around a position, for signature help: {"name", "params":
// [{"name", "type"}], "ret", "active", "doc"} or null. Types in full (the
// host shortens them).
char* glue5_checker_signature(Glue5Checker* c, const char* name, unsigned line, unsigned column) {
  try {
    c->frontend.check(name);
    const Luau::SourceModule* source = c->frontend.getSourceModule(name);
    Luau::ModulePtr module = c->frontend.moduleResolver.getModule(name);
    if (!source || !module) return owned("null");
    Luau::Position at{line, column};
    // The innermost call whose argument list holds the position.
    Luau::AstExprCall* call = nullptr;
    for (Luau::AstNode* node : Luau::findAstAncestryOfPosition(*source, at)) {
      if (auto* candidate = node->as<Luau::AstExprCall>(); candidate && candidate->func->location.end < at) {
        call = candidate;
      }
    }
    if (!call) return owned("null");
    const Luau::TypeId* found = module->astTypes.find(call->func);
    if (!found) return owned("null");
    Luau::TypeId type = Luau::follow(*found);
    // Overloads: the first.
    if (auto* both = Luau::get<Luau::IntersectionType>(type); both && !both->parts.empty()) type = Luau::follow(both->parts[0]);
    auto* f = Luau::get<Luau::FunctionType>(type);
    if (!f) return owned("null");
    std::string label;
    if (auto* index = call->func->as<Luau::AstExprIndexName>()) label = index->index.value;
    if (auto* global = call->func->as<Luau::AstExprGlobal>()) label = global->name.value;
    if (auto* local = call->func->as<Luau::AstExprLocal>()) label = local->local->name.value;
    Luau::ToStringOptions opts;
    opts.maxTypeLength = 0;
    auto [args, tail] = Luau::flatten(f->argTypes);
    // `a:b(...)` passes `a` itself first.
    size_t first = call->self ? 1 : 0;
    std::string params = "[";
    for (size_t i = first; i < args.size(); i++) {
      std::string argName = i < f->argNames.size() && f->argNames[i] ? f->argNames[i]->name : "arg" + std::to_string(i + 1);
      if (params.size() > 1) params += ",";
      params += "{\"name\":" + quote(argName) + ",\"type\":" + quote(Luau::toString(args[i], opts)) + "}";
    }
    if (tail) {
      if (params.size() > 1) params += ",";
      params += "{\"name\":\"...\",\"type\":" + quote(Luau::toString(*tail, opts)) + "}";
    }
    params += "]";
    // The argument the position is in.
    size_t active = 0;
    for (size_t i = 0; i < call->args.size; i++) {
      if (call->args.data[i]->location.end < at) active = i + 1;
    }
    if (active > 0 && active == call->args.size) {
      // After the last argument: in it, unless a comma followed it.
      const Luau::Location& last = call->args.data[active - 1]->location;
      if (!(last.end < at)) active--;
    }
    Luau::Position inName = call->func->location.end;
    if (inName.column > 0) inName.column--;
    auto doc = Luau::getDocumentationSymbolAtPosition(*source, *module, inName);
    return owned("{\"name\":" + quote(label) + ",\"params\":" + params + ",\"ret\":" + quote(Luau::toString(f->retTypes, opts)) +
                 ",\"active\":" + std::to_string(active) + ",\"doc\":" + quote(doc.value_or("")) + "}");
  } catch (const std::exception& e) {
    return owned(std::string("{\"error\":") + quote(e.what()) + "}");
  }
}

}  // extern "C"
