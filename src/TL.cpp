#include "TL.hpp"
#include "EX.hpp"
#include "LX.hpp"
#include "UT.hpp"
#include "ffi.h"
#include <cstddef>
#include <cstdio>
#include <cstring>
#include <dlfcn.h>
#include <map>
#include <stdio.h>
#include <string>
#include <unistd.h>
#include <utility>
#include <vector>

namespace TL
{

using FnMap_t = std::map<std::string, void *>;
class DFN
{
public:
  const char    *m_fn_name;
  ffi_type     **m_in_types;
  ffi_type      *m_out_type;
  static void   *m_handle;
  static FnMap_t m_fn_map;

  DFN(
    const char *fn_name, ffi_type **in_types, ffi_type *out_type)
      : m_fn_name{ fn_name },
        m_in_types{ in_types },
        m_out_type{ out_type }
  {
  }

  static void
  init(
    UT::String lib_path)
  {
    if (!m_handle) m_handle = dlopen(lib_path.m_mem, RTLD_LAZY | RTLD_LOCAL);
  }

  void
  configure(
    void)
  {
    if (m_fn_map.end() == m_fn_map.find(std::string(m_fn_name)))
    {
      void *fn_handle = dlsym(m_handle, m_fn_name);
      if (!fn_handle)
      {
        UT_FAIL_MSG("ERROR: function (%s) not found in raylib so\n", m_fn_name);
      }
      else
      {
        m_fn_map[std::string(m_fn_name)] = fn_handle;
      }
    }
    else
    {
      // Nothing to do
    }
  }

  static void
  deinit(
    void)
  {
    if (m_handle) dlclose(m_handle);
  }

  bool
  call(
    std::vector<void *> &input, void *output)
  {
    ffi_cif cif;

    void **args = input.empty() ? nullptr : input.data();

    if (ffi_prep_cif(&cif,
                     FFI_DEFAULT_ABI,
                     input.size(),
                     m_out_type,
                     input.empty() ? nullptr : m_in_types)
        != FFI_OK)
      return false;

    void *fn_handle = m_fn_map[std::string(m_fn_name)];

    ffi_call(&cif, FFI_FN(fn_handle), output, args);

    return true;
  };
};

void   *DFN::m_handle = nullptr;
FnMap_t DFN::m_fn_map = {};

static std::map<std::string, DFN *> raylib_functions = {};

std::vector<LX::LangType>
expand_signature(
  LX::Sig &sig)
{
  std::vector<LX::LangType> expansion{};

  if (LX::LangType::Fn == sig.m_type)
  {
    expansion.push_back(sig.as.m_pair.first().m_type);
    LX::Sig                   left_sig       = sig.as.m_pair.second();
    std::vector<LX::LangType> left_expansion = expand_signature(left_sig);
    expansion.insert(
      expansion.end(), left_expansion.begin(), left_expansion.end());
  }
  else
  {
    expansion.push_back(sig.m_type);
  }

  return expansion;
}

Mod::Mod(
  UT::String file_name, AR::Arena &arena)
{
  UT::String source_code = UT::read_entire_file(file_name, arena);
  this->m_defs           = { arena };
  this->m_name           = file_name;

  LX::Lexer l{ source_code.m_mem, arena, 0, source_code.m_len };
  l.run();
  l.generate_event_report();

  Env global_env{};

  for (LX::Token t : l.m_tokens)
  {
    TL::Type def_type = TL::Type::ExtDef;
    switch (t.m_type)
    {
    // TODO: this should be handled better
    case LX::Type::PubDef: def_type = TL::Type::PubDef; break;
    case LX::Type::IntDef: def_type = TL::Type::IntDef; break;
    case LX::Type::ExtDef: def_type = TL::Type::ExtDef; break;
    default              : UT_FAIL_MSG("UNREACHABLE token type: %s", UT_TCS(t.m_type));
    }

    // TODO: this should be handled better
    if (LX::Type::ExtDef == t.m_type)
    {
      LX::Sig sig = t.as.m_ext_sym.m_sig;
      DFN::init(t.as.m_ext_sym.m_def[1].as.m_string);

      if (LX::LangType::Fn == sig.m_type)
      {
        // TODO: It is assumed C functions are simple (Type, Type, Type) -> Type
        // where Type is not a function type or a structure or union
        // ie, it is a primitive, or effectively an alias to a primitive
        auto expansion = expand_signature(sig);

        size_t idx = 0;
        auto   sig_in_types
          = (ffi_type **)arena.alloc<ffi_type *>(expansion.size() - 1);
        ffi_type *sig_out_types = nullptr;

        for (size_t i = 0; i < expansion.size() - 1; ++i)
        {
          LX::LangType t = expansion[i];
          // TODO: Handle all other cases
          switch (t)
          {
          case LX::LangType::Ptr : sig_in_types[idx] = &ffi_type_pointer; break;
          case LX::LangType::Void: sig_in_types[idx] = &ffi_type_void; break;
          default                : sig_in_types[idx] = &ffi_type_sint; break;
          }
          idx += 1;
        }

        switch (expansion.back())
        {
        case LX::LangType::Ptr : sig_out_types = &ffi_type_pointer; break;
        case LX::LangType::Void: sig_out_types = &ffi_type_void; break;
        default                : sig_out_types = &ffi_type_sint; break;
        }

        auto sym = (DFN *)arena.alloc(sizeof(DFN));
        *sym     = { t.as.m_ext_sym.m_def[0].as.m_string.m_mem,
                     sig_in_types,
                     sig_out_types };

        raylib_functions[std::to_string(t.as.m_ext_sym.m_name)] = sym;
      }

      continue;
    }

    UT::String def_name   = t.as.m_sym.m_sym_name;
    LX::Tokens def_tokens = t.as.m_sym.m_def;

    EX::Parser parser{ def_tokens, arena, source_code.m_mem };
    parser.run();

    Instance instance{ *parser.m_exprs.last(), global_env };
    instance                             = eval(instance);
    global_env[std::to_string(def_name)] = instance.m_expr;
    TL::Def def{ def_type, def_name, instance.m_expr };

    this->m_defs.push(def);

    if (!true)
    {
      std::printf("%s %s = %s\n",
                  UT_TCS(def.m_type),
                  UT_TCS(def_name),
                  UT_TCS(global_env[std::to_string(def_name)]));
    }
  }

  for (auto it = global_env.begin(); it != global_env.end(); ++it)
  {
    std::printf("INFO: %s -> %s\n", it->first.c_str(), UT_TCS(it->second));
  }

  DFN::deinit();
}

static Instance
eval_bi_op(
  Instance &inst)
{
  Env      env  = inst.m_env;
  EX::Expr expr = inst.m_expr;

  Instance left_instance  = Instance{ expr.as.m_pair.first(), env };
  Instance right_instance = Instance{ expr.as.m_pair.second(), env };

  ssize_t left  = eval(left_instance).m_expr.as.m_int;
  ssize_t right = eval(right_instance).m_expr.as.m_int;

  Instance result_instance{ EX::Type::Int, env };

  switch (expr.m_type)
  {
  case EX::Type::Add    : result_instance.m_expr.as.m_int = left + right; break;
  case EX::Type::Sub    : result_instance.m_expr.as.m_int = left - right; break;
  case EX::Type::Mult   : result_instance.m_expr.as.m_int = left * right; break;
  case EX::Type::Div    : result_instance.m_expr.as.m_int = left / right; break;
  case EX::Type::Modulus: result_instance.m_expr.as.m_int = left % right; break;
  case EX::Type::IsEq   : result_instance.m_expr.as.m_int = left == right; break;
  default               : UT_FAIL_MSG("UNREACHABLE expr.m_type = %s", expr.m_type);
  }
  return result_instance;
}

Instance
eval(
  Instance &inst)
{
  EX::Expr expr = inst.m_expr;
  Env      env  = inst.m_env;

  switch (expr.m_type)
  {
  case EX::Type::Add:
  case EX::Type::Sub:
  case EX::Type::Mult:
  case EX::Type::Div:
  case EX::Type::Modulus:
  case EX::Type::IsEq   : return eval_bi_op(inst);
  case EX::Type::Int    : return inst;
  case EX::Type::Var:
  {
    UT::String var_name = expr.as.m_var;
    auto       var_expr = env.find(std::to_string(var_name));

    if (var_expr != env.end())
    {
      Instance new_instance{ var_expr->second, env };
      return new_instance;
    }
    else if (raylib_functions.end()
             != raylib_functions.find(std::to_string(var_name)))
    {
      DFN raylib_fn = *raylib_functions.find(var_name.m_mem)->second;
      raylib_fn.configure();
      AR::Arena output_arena{};

      // FIXME: assume output fits in 64 bytes
      void *output = output_arena.alloc(64);

      // The output might never be written to so 0 init
      std::memset(output, 0, 64);
      std::vector<void *> _input;

      raylib_fn.call(_input, output);

      // FIXME: Don't assume the function only returns ints
      EX::Expr int_expr{ EX::Type::Int };
      int_expr.as.m_int = *(ssize_t *)output;

      Instance new_instance{ int_expr, env };
      return new_instance;
    }
    else
    {
      // FIXME: We should not fail like that
      UT_FAIL_MSG("Variable (%s) is not defined", UT_TCS(var_name));
    }
  }
  break;
  case EX::Type::Minus:
  {
    Instance new_instance{ *expr.as.m_expr, env };
    new_instance = eval(new_instance);
    // TODO: this assumes the expression evaluates to an int, which is not
    // always the case
    new_instance.m_expr.as.m_int *= -1;
    return new_instance;
  }
  break;
  case EX::Type::FnApp:
  {
    EX::Exprs params = expr.as.m_fnapp.m_param;
    EX::Expr  fndef  = expr;
    Env       env    = inst.m_env;

    for (EX::Expr &param_expr : params)
    {
      Instance param_inst{ param_expr, env };
      env[std::to_string(fndef.as.m_fn.m_param)] = eval(param_inst).m_expr;
      fndef                                      = *fndef.as.m_fn.m_body;
    }

    Instance body_instance{ fndef, env };
    body_instance = eval(body_instance);

    return body_instance;
  }
  case EX::Type::FnDef:
  {
    return inst;
  }
  case EX::Type::VarApp:
  {
    std::string fn_name   = std::to_string(expr.as.m_varapp.m_fn_name);
    auto        fn_def_it = env.find(fn_name);
    EX::Expr    fndef{};

    auto is_raylib = raylib_functions.find(fn_name.c_str());

    if (env.end() != fn_def_it)
    {
      fndef            = fn_def_it->second;
      EX::Exprs params = expr.as.m_varapp.m_param;
      for (EX::Expr &param_expr : params)
      {
        Instance param_inst{ param_expr, env };
        env[std::to_string(fndef.as.m_fn.m_param)] = eval(param_inst).m_expr;
        fndef                                      = *fndef.as.m_fn.m_body;
      }

      Instance app_instance{ fndef, env };
      app_instance = eval(app_instance);

      return app_instance;
    }
    // TODO: There should be a better way to both load and define functions
    else if (raylib_functions.end() != is_raylib)
    {
      DFN raylib_fn = *raylib_functions.find(fn_name.c_str())->second;
      raylib_fn.configure();

      AR::Arena           input_buffer{};
      std::vector<void *> input;

      // FIXME: assume output fits in 64 bytes
      void *output = input_buffer.alloc(64);

      EX::Exprs params = expr.as.m_varapp.m_param;
      for (auto &param_expr : params)
      {
        Instance param_inst{ param_expr, env };
        param_inst = eval(param_inst);

        if (EX::Type::Int == param_inst.m_expr.m_type)
        {
          ssize_t param = param_inst.m_expr.as.m_int;

          void *param_buffer      = input_buffer.alloc<ssize_t>(1);
          *(size_t *)param_buffer = param;
          input.push_back(param_buffer);
        }
        else if (EX::Type::Str == param_inst.m_expr.m_type)
        {
          char *param = param_inst.m_expr.as.m_string.m_mem;

          void *param_buffer     = input_buffer.alloc<ssize_t>(1);
          *(char **)param_buffer = param;
          input.push_back(param_buffer);
        }
        else
        {
          UT_TODO();
        }
      }

      bool ok = raylib_fn.call(input, output);
      (void)ok;

      Instance app_instance{ fndef, env };
      app_instance.m_expr.m_type   = EX::Type::Int;
      app_instance.m_expr.as.m_int = *(ssize_t *)output;

      return app_instance;
    }
    else
    {
      // TODO: Use DFN class
      void *handle = dlopen("./bin/bc.so", RTLD_LAZY | RTLD_DEEPBIND);
      void *fn     = dlsym(handle, fn_name.c_str());

      int ret = 0;

      EX::Expr *app_param = expr.as.m_varapp.m_param.last();
      ssize_t   param
        = EX::Type::Var == app_param->m_type
            ? (ssize_t)env[std::string(app_param->as.m_varapp.m_fn_name.m_mem)]
                .as.m_string.m_mem
            : app_param->as.m_int;

      /* libffi setup */
      ffi_cif   cif;
      ffi_type *arg_types[1] = { &ffi_type_sint64 };
      ffi_type *ret_type     = &ffi_type_sint;

      ssize_t ffi_result
        = ffi_prep_cif(&cif, FFI_DEFAULT_ABI, 1, ret_type, arg_types);

      if (FFI_OK != ffi_result)
      {
        UT_FAIL_MSG("LIB FFI call failed with %zu\n", ffi_result);
        // FIXME: handle error
      }

      void *args[1] = { &param };
      ffi_call(&cif, FFI_FN(fn), &ret, args);

      dlclose(handle);

      Instance app_instance{ fndef, env };
      app_instance.m_expr.m_type   = EX::Type::Int;
      app_instance.m_expr.as.m_int = ret;

      return app_instance;
    }
  }
  case EX::Type::If:
  {
    Instance cond_instance{ *expr.as.m_if.m_condition, env };
    Instance true_instance{ *expr.as.m_if.m_true_branch, env };
    Instance else_instance{ *expr.as.m_if.m_else_branch, env };

    return eval(cond_instance).m_expr.as.m_int ? eval(true_instance)
                                               : eval(else_instance);
  }
  case EX::Type::Let:
  {
    UT::String var_name = expr.as.m_let.m_var_name;
    Instance   value_instance{ *expr.as.m_let.m_value, env };
    value_instance = eval(value_instance);

    Env local_env                       = env;
    local_env[std::to_string(var_name)] = value_instance.m_expr;

    Instance continuation_instrance{ *expr.as.m_let.m_continuation, local_env };

    return eval(continuation_instrance);
  }
  case EX::Type::Not:
  {
    Instance inner_instance{ *expr.as.m_expr, env };
    inner_instance       = eval(inner_instance);
    ssize_t &inner_value = inner_instance.m_expr.as.m_int;
    inner_value          = not inner_value;

    return inner_instance;
  }
  case EX::Type::Str:
  {
    return inst;
  }
  break;
  case EX::Type::While:
  {
    Instance return_instance        = { EX::Expr{ EX::Type::Int }, env };
    Env      while_env              = env;
    return_instance.m_expr.as.m_int = 0;

    EX::Expr condition_expr = *expr.as.m_while.m_condition;
    EX::Expr body_expr      = *expr.as.m_while.m_body;
    Instance condition_instance{ condition_expr, while_env };
    Instance body_instance{ body_expr, while_env };

  TL_CONDITION_BLOCK:
  {
    condition_instance = { condition_expr, while_env };
    condition_instance = eval(condition_instance);
    while_env          = condition_instance.m_env;
    if (EX::Type::Int != condition_instance.m_expr.m_type)
    {
      UT_FAIL_MSG("Expected integer(bool) but found %s\n", UT_TCS(expr.m_type));
    }
    bool should_loop = condition_instance.m_expr.as.m_int;

    if (should_loop)
      goto TL_BODY_EVAL_BLOCK;
    else
      goto TL_RETURN_BLOCK;
  }

  TL_BODY_EVAL_BLOCK:
  {
    body_instance = { body_expr, while_env };
    body_instance = eval(body_instance);
    while_env     = body_instance.m_env;

    goto TL_CONDITION_BLOCK;
  }

  TL_RETURN_BLOCK:
    return return_instance;
  }
  break;
  case EX::Type::Unknown:
  default:
  {
    UT_FAIL_MSG("Type <%s> not supported yet\n", UT_TCS(expr.m_type));
  }
  break;
  }

  UT_FAIL_MSG("Expr type not resolved, type = %s", expr.m_type);
  return Instance{};
}
} // namespace TL
