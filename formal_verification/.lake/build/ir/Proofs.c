// Lean compiler output
// Module: Proofs
// Imports: Init Proofs.AccessControl Proofs.Conservation Proofs.HealthInvariants Proofs.ArithmeticSafety Proofs.LiquidationCorrectness Proofs.FundingSymmetry Proofs.FeeMonotonicity
#include <lean/lean.h>
#if defined(__clang__)
#pragma clang diagnostic ignored "-Wunused-parameter"
#pragma clang diagnostic ignored "-Wunused-label"
#elif defined(__GNUC__) && !defined(__CLANG__)
#pragma GCC diagnostic ignored "-Wunused-parameter"
#pragma GCC diagnostic ignored "-Wunused-label"
#pragma GCC diagnostic ignored "-Wunused-but-set-variable"
#endif
#ifdef __cplusplus
extern "C" {
#endif
lean_object* initialize_Init(uint8_t builtin, lean_object*);
lean_object* initialize_Proofs_AccessControl(uint8_t builtin, lean_object*);
lean_object* initialize_Proofs_Conservation(uint8_t builtin, lean_object*);
lean_object* initialize_Proofs_HealthInvariants(uint8_t builtin, lean_object*);
lean_object* initialize_Proofs_ArithmeticSafety(uint8_t builtin, lean_object*);
lean_object* initialize_Proofs_LiquidationCorrectness(uint8_t builtin, lean_object*);
lean_object* initialize_Proofs_FundingSymmetry(uint8_t builtin, lean_object*);
lean_object* initialize_Proofs_FeeMonotonicity(uint8_t builtin, lean_object*);
static bool _G_initialized = false;
LEAN_EXPORT lean_object* initialize_Proofs(uint8_t builtin, lean_object* w) {
lean_object * res;
if (_G_initialized) return lean_io_result_mk_ok(lean_box(0));
_G_initialized = true;
res = initialize_Init(builtin, lean_io_mk_world());
if (lean_io_result_is_error(res)) return res;
lean_dec_ref(res);
res = initialize_Proofs_AccessControl(builtin, lean_io_mk_world());
if (lean_io_result_is_error(res)) return res;
lean_dec_ref(res);
res = initialize_Proofs_Conservation(builtin, lean_io_mk_world());
if (lean_io_result_is_error(res)) return res;
lean_dec_ref(res);
res = initialize_Proofs_HealthInvariants(builtin, lean_io_mk_world());
if (lean_io_result_is_error(res)) return res;
lean_dec_ref(res);
res = initialize_Proofs_ArithmeticSafety(builtin, lean_io_mk_world());
if (lean_io_result_is_error(res)) return res;
lean_dec_ref(res);
res = initialize_Proofs_LiquidationCorrectness(builtin, lean_io_mk_world());
if (lean_io_result_is_error(res)) return res;
lean_dec_ref(res);
res = initialize_Proofs_FundingSymmetry(builtin, lean_io_mk_world());
if (lean_io_result_is_error(res)) return res;
lean_dec_ref(res);
res = initialize_Proofs_FeeMonotonicity(builtin, lean_io_mk_world());
if (lean_io_result_is_error(res)) return res;
lean_dec_ref(res);
return lean_io_result_mk_ok(lean_box(0));
}
#ifdef __cplusplus
}
#endif
