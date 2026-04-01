// Lean compiler output
// Module: MangoState.PerpMarket
// Imports: Init MangoState.Types QEDGen.Solana
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
static lean_object* l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__24;
static lean_object* l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__19;
static lean_object* l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__62;
static lean_object* l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__6;
static lean_object* l___private_MangoState_PerpMarket_0__MangoV4_reprPerpPosition____x40_MangoState_PerpMarket___hyg_829____closed__7;
static lean_object* l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__26;
static lean_object* l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__50;
LEAN_EXPORT lean_object* l_MangoV4_perpSocializeLossTransition(lean_object*, lean_object*);
static lean_object* l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__49;
static lean_object* l___private_MangoState_PerpMarket_0__MangoV4_reprPerpPosition____x40_MangoState_PerpMarket___hyg_829____closed__1;
static lean_object* l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__61;
static lean_object* l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__8;
static lean_object* l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__82;
static lean_object* l___private_MangoState_PerpMarket_0__MangoV4_reprPerpPosition____x40_MangoState_PerpMarket___hyg_829____closed__25;
static lean_object* l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__53;
static lean_object* l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__77;
static lean_object* l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__23;
static lean_object* l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__76;
static lean_object* l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__66;
static lean_object* l___private_MangoState_PerpMarket_0__MangoV4_reprPerpPosition____x40_MangoState_PerpMarket___hyg_829____closed__17;
static lean_object* l___private_MangoState_PerpMarket_0__MangoV4_reprPerpPosition____x40_MangoState_PerpMarket___hyg_829____closed__13;
LEAN_EXPORT lean_object* l___private_MangoState_PerpMarket_0__MangoV4_reprPerpPosition____x40_MangoState_PerpMarket___hyg_829____boxed(lean_object*, lean_object*);
static lean_object* l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__14;
static lean_object* l___private_MangoState_PerpMarket_0__MangoV4_reprPerpPosition____x40_MangoState_PerpMarket___hyg_829____closed__19;
static lean_object* l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__78;
static lean_object* l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__7;
static lean_object* l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__2;
static lean_object* l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__58;
LEAN_EXPORT lean_object* l_MangoV4_instReprPerpMarket;
static lean_object* l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__15;
static lean_object* l___private_MangoState_PerpMarket_0__MangoV4_reprPerpPosition____x40_MangoState_PerpMarket___hyg_829____closed__3;
static lean_object* l___private_MangoState_PerpMarket_0__MangoV4_reprPerpPosition____x40_MangoState_PerpMarket___hyg_829____closed__30;
static lean_object* l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__52;
static lean_object* l___private_MangoState_PerpMarket_0__MangoV4_reprPerpPosition____x40_MangoState_PerpMarket___hyg_829____closed__23;
uint8_t lean_int_dec_le(lean_object*, lean_object*);
static lean_object* l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__28;
static lean_object* l___private_MangoState_PerpMarket_0__MangoV4_reprPerpPosition____x40_MangoState_PerpMarket___hyg_829____closed__6;
static lean_object* l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__3;
static lean_object* l___private_MangoState_PerpMarket_0__MangoV4_reprPerpPosition____x40_MangoState_PerpMarket___hyg_829____closed__15;
static lean_object* l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__34;
static lean_object* l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__75;
static lean_object* l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__29;
static lean_object* l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__45;
static lean_object* l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__54;
static lean_object* l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__69;
static lean_object* l___private_MangoState_PerpMarket_0__MangoV4_reprPerpPosition____x40_MangoState_PerpMarket___hyg_829____closed__29;
static lean_object* l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__30;
static lean_object* l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__39;
static lean_object* l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__74;
static lean_object* l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__36;
static lean_object* l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__11;
LEAN_EXPORT lean_object* l_MangoV4_fundingUpdateTransition(lean_object*, lean_object*, lean_object*, lean_object*);
static lean_object* l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__25;
lean_object* lean_nat_to_int(lean_object*);
static lean_object* l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__48;
static lean_object* l___private_MangoState_PerpMarket_0__MangoV4_reprPerpPosition____x40_MangoState_PerpMarket___hyg_829____closed__18;
static lean_object* l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__9;
static lean_object* l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__72;
static lean_object* l___private_MangoState_PerpMarket_0__MangoV4_reprPerpPosition____x40_MangoState_PerpMarket___hyg_829____closed__24;
static lean_object* l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__20;
static lean_object* l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__47;
static lean_object* l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__43;
static lean_object* l___private_MangoState_PerpMarket_0__MangoV4_reprPerpPosition____x40_MangoState_PerpMarket___hyg_829____closed__26;
static lean_object* l___private_MangoState_PerpMarket_0__MangoV4_reprPerpPosition____x40_MangoState_PerpMarket___hyg_829____closed__32;
lean_object* l_Int_repr(lean_object*);
static lean_object* l___private_MangoState_PerpMarket_0__MangoV4_reprPerpPosition____x40_MangoState_PerpMarket___hyg_829____closed__31;
static lean_object* l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__10;
LEAN_EXPORT lean_object* l_MangoV4_instReprPerpPosition;
LEAN_EXPORT lean_object* l_MangoV4_perpSocializeLossTransition___boxed(lean_object*, lean_object*);
static lean_object* l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__1;
static lean_object* l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__35;
static lean_object* l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__65;
static lean_object* l___private_MangoState_PerpMarket_0__MangoV4_reprPerpPosition____x40_MangoState_PerpMarket___hyg_829____closed__4;
static lean_object* l___private_MangoState_PerpMarket_0__MangoV4_reprPerpPosition____x40_MangoState_PerpMarket___hyg_829____closed__11;
static lean_object* l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__13;
LEAN_EXPORT lean_object* l_MangoV4_feeAccrualTransition___boxed(lean_object*, lean_object*, lean_object*);
LEAN_EXPORT lean_object* l___private_MangoState_PerpMarket_0__MangoV4_reprPerpPosition____x40_MangoState_PerpMarket___hyg_829_(lean_object*, lean_object*);
static lean_object* l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__63;
static lean_object* l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__21;
LEAN_EXPORT lean_object* l_MangoV4_placeOrderTransition(lean_object*);
static lean_object* l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__68;
static lean_object* l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__71;
static lean_object* l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__51;
lean_object* lean_int_sub(lean_object*, lean_object*);
static lean_object* l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__70;
static lean_object* l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__81;
static lean_object* l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__37;
static lean_object* l___private_MangoState_PerpMarket_0__MangoV4_reprPerpPosition____x40_MangoState_PerpMarket___hyg_829____closed__5;
static lean_object* l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__57;
static lean_object* l_MangoV4_instReprPerpPosition___closed__1;
static lean_object* l___private_MangoState_PerpMarket_0__MangoV4_reprPerpPosition____x40_MangoState_PerpMarket___hyg_829____closed__22;
static lean_object* l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__64;
static lean_object* l___private_MangoState_PerpMarket_0__MangoV4_reprPerpPosition____x40_MangoState_PerpMarket___hyg_829____closed__27;
static lean_object* l___private_MangoState_PerpMarket_0__MangoV4_reprPerpPosition____x40_MangoState_PerpMarket___hyg_829____closed__21;
static lean_object* l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__59;
static lean_object* l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__4;
static lean_object* l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__5;
lean_object* lean_int_mul(lean_object*, lean_object*);
lean_object* lean_string_length(lean_object*);
static lean_object* l___private_MangoState_PerpMarket_0__MangoV4_reprPerpPosition____x40_MangoState_PerpMarket___hyg_829____closed__8;
static lean_object* l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__41;
static lean_object* l___private_MangoState_PerpMarket_0__MangoV4_reprPerpPosition____x40_MangoState_PerpMarket___hyg_829____closed__14;
uint8_t lean_nat_dec_lt(lean_object*, lean_object*);
static lean_object* l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__60;
LEAN_EXPORT lean_object* l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____boxed(lean_object*, lean_object*);
static lean_object* l_MangoV4_instReprPerpMarket___closed__1;
static lean_object* l___private_MangoState_PerpMarket_0__MangoV4_reprPerpPosition____x40_MangoState_PerpMarket___hyg_829____closed__2;
lean_object* l_Repr_addAppParen(lean_object*, lean_object*);
static lean_object* l___private_MangoState_PerpMarket_0__MangoV4_reprPerpPosition____x40_MangoState_PerpMarket___hyg_829____closed__16;
uint8_t lean_int_dec_lt(lean_object*, lean_object*);
static lean_object* l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__38;
LEAN_EXPORT lean_object* l_MangoV4_fundingUpdateTransition___boxed(lean_object*, lean_object*, lean_object*, lean_object*);
static lean_object* l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__46;
static lean_object* l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__67;
static lean_object* l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__83;
static lean_object* l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__16;
static lean_object* l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__17;
static lean_object* l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__73;
static lean_object* l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__33;
static lean_object* l___private_MangoState_PerpMarket_0__MangoV4_reprPerpPosition____x40_MangoState_PerpMarket___hyg_829____closed__28;
static lean_object* l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__31;
static lean_object* l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__27;
static lean_object* l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__84;
lean_object* lean_int_add(lean_object*, lean_object*);
static lean_object* l___private_MangoState_PerpMarket_0__MangoV4_reprPerpPosition____x40_MangoState_PerpMarket___hyg_829____closed__10;
static lean_object* l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__55;
static lean_object* l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__42;
static lean_object* l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__79;
static lean_object* l___private_MangoState_PerpMarket_0__MangoV4_reprPerpPosition____x40_MangoState_PerpMarket___hyg_829____closed__20;
lean_object* lean_int_ediv(lean_object*, lean_object*);
static lean_object* l___private_MangoState_PerpMarket_0__MangoV4_reprPerpPosition____x40_MangoState_PerpMarket___hyg_829____closed__12;
uint8_t lean_nat_dec_le(lean_object*, lean_object*);
static lean_object* l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__22;
static lean_object* l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__12;
lean_object* lean_nat_add(lean_object*, lean_object*);
static lean_object* l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__44;
LEAN_EXPORT lean_object* l_MangoV4_feeAccrualTransition(lean_object*, lean_object*, lean_object*);
LEAN_EXPORT lean_object* l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210_(lean_object*, lean_object*);
static lean_object* l___private_MangoState_PerpMarket_0__MangoV4_reprPerpPosition____x40_MangoState_PerpMarket___hyg_829____closed__9;
static lean_object* l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__18;
static lean_object* l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__40;
lean_object* l___private_Init_Data_Repr_0__Nat_reprFast(lean_object*);
static lean_object* l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__80;
static lean_object* l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__56;
static lean_object* l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__32;
static lean_object* _init_l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__1() {
_start:
{
lean_object* x_1; 
x_1 = lean_mk_string_unchecked("group", 5, 5);
return x_1;
}
}
static lean_object* _init_l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__2() {
_start:
{
lean_object* x_1; lean_object* x_2; 
x_1 = l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__1;
x_2 = lean_alloc_ctor(3, 1, 0);
lean_ctor_set(x_2, 0, x_1);
return x_2;
}
}
static lean_object* _init_l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__3() {
_start:
{
lean_object* x_1; lean_object* x_2; lean_object* x_3; 
x_1 = lean_box(0);
x_2 = l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__2;
x_3 = lean_alloc_ctor(5, 2, 0);
lean_ctor_set(x_3, 0, x_1);
lean_ctor_set(x_3, 1, x_2);
return x_3;
}
}
static lean_object* _init_l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__4() {
_start:
{
lean_object* x_1; 
x_1 = lean_mk_string_unchecked(" := ", 4, 4);
return x_1;
}
}
static lean_object* _init_l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__5() {
_start:
{
lean_object* x_1; lean_object* x_2; 
x_1 = l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__4;
x_2 = lean_alloc_ctor(3, 1, 0);
lean_ctor_set(x_2, 0, x_1);
return x_2;
}
}
static lean_object* _init_l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__6() {
_start:
{
lean_object* x_1; lean_object* x_2; lean_object* x_3; 
x_1 = l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__3;
x_2 = l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__5;
x_3 = lean_alloc_ctor(5, 2, 0);
lean_ctor_set(x_3, 0, x_1);
lean_ctor_set(x_3, 1, x_2);
return x_3;
}
}
static lean_object* _init_l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__7() {
_start:
{
lean_object* x_1; lean_object* x_2; 
x_1 = lean_unsigned_to_nat(9u);
x_2 = lean_nat_to_int(x_1);
return x_2;
}
}
static lean_object* _init_l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__8() {
_start:
{
lean_object* x_1; 
x_1 = lean_mk_string_unchecked(",", 1, 1);
return x_1;
}
}
static lean_object* _init_l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__9() {
_start:
{
lean_object* x_1; lean_object* x_2; 
x_1 = l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__8;
x_2 = lean_alloc_ctor(3, 1, 0);
lean_ctor_set(x_2, 0, x_1);
return x_2;
}
}
static lean_object* _init_l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__10() {
_start:
{
lean_object* x_1; 
x_1 = lean_mk_string_unchecked("settle_token_index", 18, 18);
return x_1;
}
}
static lean_object* _init_l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__11() {
_start:
{
lean_object* x_1; lean_object* x_2; 
x_1 = l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__10;
x_2 = lean_alloc_ctor(3, 1, 0);
lean_ctor_set(x_2, 0, x_1);
return x_2;
}
}
static lean_object* _init_l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__12() {
_start:
{
lean_object* x_1; lean_object* x_2; 
x_1 = lean_unsigned_to_nat(22u);
x_2 = lean_nat_to_int(x_1);
return x_2;
}
}
static lean_object* _init_l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__13() {
_start:
{
lean_object* x_1; 
x_1 = lean_mk_string_unchecked("perp_market_index", 17, 17);
return x_1;
}
}
static lean_object* _init_l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__14() {
_start:
{
lean_object* x_1; lean_object* x_2; 
x_1 = l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__13;
x_2 = lean_alloc_ctor(3, 1, 0);
lean_ctor_set(x_2, 0, x_1);
return x_2;
}
}
static lean_object* _init_l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__15() {
_start:
{
lean_object* x_1; lean_object* x_2; 
x_1 = lean_unsigned_to_nat(21u);
x_2 = lean_nat_to_int(x_1);
return x_2;
}
}
static lean_object* _init_l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__16() {
_start:
{
lean_object* x_1; 
x_1 = lean_mk_string_unchecked("oracle", 6, 6);
return x_1;
}
}
static lean_object* _init_l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__17() {
_start:
{
lean_object* x_1; lean_object* x_2; 
x_1 = l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__16;
x_2 = lean_alloc_ctor(3, 1, 0);
lean_ctor_set(x_2, 0, x_1);
return x_2;
}
}
static lean_object* _init_l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__18() {
_start:
{
lean_object* x_1; lean_object* x_2; 
x_1 = lean_unsigned_to_nat(10u);
x_2 = lean_nat_to_int(x_1);
return x_2;
}
}
static lean_object* _init_l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__19() {
_start:
{
lean_object* x_1; 
x_1 = lean_mk_string_unchecked("quote_lot_size", 14, 14);
return x_1;
}
}
static lean_object* _init_l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__20() {
_start:
{
lean_object* x_1; lean_object* x_2; 
x_1 = l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__19;
x_2 = lean_alloc_ctor(3, 1, 0);
lean_ctor_set(x_2, 0, x_1);
return x_2;
}
}
static lean_object* _init_l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__21() {
_start:
{
lean_object* x_1; lean_object* x_2; 
x_1 = lean_unsigned_to_nat(18u);
x_2 = lean_nat_to_int(x_1);
return x_2;
}
}
static lean_object* _init_l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__22() {
_start:
{
lean_object* x_1; 
x_1 = lean_mk_string_unchecked("base_lot_size", 13, 13);
return x_1;
}
}
static lean_object* _init_l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__23() {
_start:
{
lean_object* x_1; lean_object* x_2; 
x_1 = l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__22;
x_2 = lean_alloc_ctor(3, 1, 0);
lean_ctor_set(x_2, 0, x_1);
return x_2;
}
}
static lean_object* _init_l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__24() {
_start:
{
lean_object* x_1; lean_object* x_2; 
x_1 = lean_unsigned_to_nat(17u);
x_2 = lean_nat_to_int(x_1);
return x_2;
}
}
static lean_object* _init_l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__25() {
_start:
{
lean_object* x_1; 
x_1 = lean_mk_string_unchecked("long_funding", 12, 12);
return x_1;
}
}
static lean_object* _init_l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__26() {
_start:
{
lean_object* x_1; lean_object* x_2; 
x_1 = l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__25;
x_2 = lean_alloc_ctor(3, 1, 0);
lean_ctor_set(x_2, 0, x_1);
return x_2;
}
}
static lean_object* _init_l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__27() {
_start:
{
lean_object* x_1; lean_object* x_2; 
x_1 = lean_unsigned_to_nat(16u);
x_2 = lean_nat_to_int(x_1);
return x_2;
}
}
static lean_object* _init_l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__28() {
_start:
{
lean_object* x_1; lean_object* x_2; 
x_1 = lean_unsigned_to_nat(0u);
x_2 = lean_nat_to_int(x_1);
return x_2;
}
}
static lean_object* _init_l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__29() {
_start:
{
lean_object* x_1; 
x_1 = lean_mk_string_unchecked("short_funding", 13, 13);
return x_1;
}
}
static lean_object* _init_l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__30() {
_start:
{
lean_object* x_1; lean_object* x_2; 
x_1 = l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__29;
x_2 = lean_alloc_ctor(3, 1, 0);
lean_ctor_set(x_2, 0, x_1);
return x_2;
}
}
static lean_object* _init_l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__31() {
_start:
{
lean_object* x_1; 
x_1 = lean_mk_string_unchecked("funding_last_updated", 20, 20);
return x_1;
}
}
static lean_object* _init_l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__32() {
_start:
{
lean_object* x_1; lean_object* x_2; 
x_1 = l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__31;
x_2 = lean_alloc_ctor(3, 1, 0);
lean_ctor_set(x_2, 0, x_1);
return x_2;
}
}
static lean_object* _init_l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__33() {
_start:
{
lean_object* x_1; lean_object* x_2; 
x_1 = lean_unsigned_to_nat(24u);
x_2 = lean_nat_to_int(x_1);
return x_2;
}
}
static lean_object* _init_l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__34() {
_start:
{
lean_object* x_1; 
x_1 = lean_mk_string_unchecked("min_funding", 11, 11);
return x_1;
}
}
static lean_object* _init_l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__35() {
_start:
{
lean_object* x_1; lean_object* x_2; 
x_1 = l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__34;
x_2 = lean_alloc_ctor(3, 1, 0);
lean_ctor_set(x_2, 0, x_1);
return x_2;
}
}
static lean_object* _init_l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__36() {
_start:
{
lean_object* x_1; lean_object* x_2; 
x_1 = lean_unsigned_to_nat(15u);
x_2 = lean_nat_to_int(x_1);
return x_2;
}
}
static lean_object* _init_l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__37() {
_start:
{
lean_object* x_1; 
x_1 = lean_mk_string_unchecked("max_funding", 11, 11);
return x_1;
}
}
static lean_object* _init_l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__38() {
_start:
{
lean_object* x_1; lean_object* x_2; 
x_1 = l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__37;
x_2 = lean_alloc_ctor(3, 1, 0);
lean_ctor_set(x_2, 0, x_1);
return x_2;
}
}
static lean_object* _init_l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__39() {
_start:
{
lean_object* x_1; 
x_1 = lean_mk_string_unchecked("open_interest", 13, 13);
return x_1;
}
}
static lean_object* _init_l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__40() {
_start:
{
lean_object* x_1; lean_object* x_2; 
x_1 = l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__39;
x_2 = lean_alloc_ctor(3, 1, 0);
lean_ctor_set(x_2, 0, x_1);
return x_2;
}
}
static lean_object* _init_l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__41() {
_start:
{
lean_object* x_1; 
x_1 = lean_mk_string_unchecked("seq_num", 7, 7);
return x_1;
}
}
static lean_object* _init_l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__42() {
_start:
{
lean_object* x_1; lean_object* x_2; 
x_1 = l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__41;
x_2 = lean_alloc_ctor(3, 1, 0);
lean_ctor_set(x_2, 0, x_1);
return x_2;
}
}
static lean_object* _init_l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__43() {
_start:
{
lean_object* x_1; lean_object* x_2; 
x_1 = lean_unsigned_to_nat(11u);
x_2 = lean_nat_to_int(x_1);
return x_2;
}
}
static lean_object* _init_l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__44() {
_start:
{
lean_object* x_1; 
x_1 = lean_mk_string_unchecked("maker_fee", 9, 9);
return x_1;
}
}
static lean_object* _init_l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__45() {
_start:
{
lean_object* x_1; lean_object* x_2; 
x_1 = l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__44;
x_2 = lean_alloc_ctor(3, 1, 0);
lean_ctor_set(x_2, 0, x_1);
return x_2;
}
}
static lean_object* _init_l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__46() {
_start:
{
lean_object* x_1; lean_object* x_2; 
x_1 = lean_unsigned_to_nat(13u);
x_2 = lean_nat_to_int(x_1);
return x_2;
}
}
static lean_object* _init_l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__47() {
_start:
{
lean_object* x_1; 
x_1 = lean_mk_string_unchecked("taker_fee", 9, 9);
return x_1;
}
}
static lean_object* _init_l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__48() {
_start:
{
lean_object* x_1; lean_object* x_2; 
x_1 = l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__47;
x_2 = lean_alloc_ctor(3, 1, 0);
lean_ctor_set(x_2, 0, x_1);
return x_2;
}
}
static lean_object* _init_l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__49() {
_start:
{
lean_object* x_1; 
x_1 = lean_mk_string_unchecked("fees_accrued", 12, 12);
return x_1;
}
}
static lean_object* _init_l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__50() {
_start:
{
lean_object* x_1; lean_object* x_2; 
x_1 = l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__49;
x_2 = lean_alloc_ctor(3, 1, 0);
lean_ctor_set(x_2, 0, x_1);
return x_2;
}
}
static lean_object* _init_l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__51() {
_start:
{
lean_object* x_1; 
x_1 = lean_mk_string_unchecked("fees_settled", 12, 12);
return x_1;
}
}
static lean_object* _init_l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__52() {
_start:
{
lean_object* x_1; lean_object* x_2; 
x_1 = l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__51;
x_2 = lean_alloc_ctor(3, 1, 0);
lean_ctor_set(x_2, 0, x_1);
return x_2;
}
}
static lean_object* _init_l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__53() {
_start:
{
lean_object* x_1; 
x_1 = lean_mk_string_unchecked("base_liquidation_fee", 20, 20);
return x_1;
}
}
static lean_object* _init_l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__54() {
_start:
{
lean_object* x_1; lean_object* x_2; 
x_1 = l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__53;
x_2 = lean_alloc_ctor(3, 1, 0);
lean_ctor_set(x_2, 0, x_1);
return x_2;
}
}
static lean_object* _init_l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__55() {
_start:
{
lean_object* x_1; 
x_1 = lean_mk_string_unchecked("platform_liquidation_fee", 24, 24);
return x_1;
}
}
static lean_object* _init_l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__56() {
_start:
{
lean_object* x_1; lean_object* x_2; 
x_1 = l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__55;
x_2 = lean_alloc_ctor(3, 1, 0);
lean_ctor_set(x_2, 0, x_1);
return x_2;
}
}
static lean_object* _init_l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__57() {
_start:
{
lean_object* x_1; lean_object* x_2; 
x_1 = lean_unsigned_to_nat(28u);
x_2 = lean_nat_to_int(x_1);
return x_2;
}
}
static lean_object* _init_l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__58() {
_start:
{
lean_object* x_1; 
x_1 = lean_mk_string_unchecked("accrued_liquidation_fees", 24, 24);
return x_1;
}
}
static lean_object* _init_l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__59() {
_start:
{
lean_object* x_1; lean_object* x_2; 
x_1 = l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__58;
x_2 = lean_alloc_ctor(3, 1, 0);
lean_ctor_set(x_2, 0, x_1);
return x_2;
}
}
static lean_object* _init_l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__60() {
_start:
{
lean_object* x_1; 
x_1 = lean_mk_string_unchecked("init_overall_asset_weight", 25, 25);
return x_1;
}
}
static lean_object* _init_l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__61() {
_start:
{
lean_object* x_1; lean_object* x_2; 
x_1 = l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__60;
x_2 = lean_alloc_ctor(3, 1, 0);
lean_ctor_set(x_2, 0, x_1);
return x_2;
}
}
static lean_object* _init_l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__62() {
_start:
{
lean_object* x_1; lean_object* x_2; 
x_1 = lean_unsigned_to_nat(29u);
x_2 = lean_nat_to_int(x_1);
return x_2;
}
}
static lean_object* _init_l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__63() {
_start:
{
lean_object* x_1; 
x_1 = lean_mk_string_unchecked("unsocialized_loss", 17, 17);
return x_1;
}
}
static lean_object* _init_l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__64() {
_start:
{
lean_object* x_1; lean_object* x_2; 
x_1 = l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__63;
x_2 = lean_alloc_ctor(3, 1, 0);
lean_ctor_set(x_2, 0, x_1);
return x_2;
}
}
static lean_object* _init_l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__65() {
_start:
{
lean_object* x_1; 
x_1 = lean_mk_string_unchecked("reduce_only", 11, 11);
return x_1;
}
}
static lean_object* _init_l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__66() {
_start:
{
lean_object* x_1; lean_object* x_2; 
x_1 = l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__65;
x_2 = lean_alloc_ctor(3, 1, 0);
lean_ctor_set(x_2, 0, x_1);
return x_2;
}
}
static lean_object* _init_l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__67() {
_start:
{
lean_object* x_1; 
x_1 = lean_mk_string_unchecked("force_close", 11, 11);
return x_1;
}
}
static lean_object* _init_l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__68() {
_start:
{
lean_object* x_1; lean_object* x_2; 
x_1 = l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__67;
x_2 = lean_alloc_ctor(3, 1, 0);
lean_ctor_set(x_2, 0, x_1);
return x_2;
}
}
static lean_object* _init_l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__69() {
_start:
{
lean_object* x_1; 
x_1 = lean_mk_string_unchecked("group_insurance_fund", 20, 20);
return x_1;
}
}
static lean_object* _init_l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__70() {
_start:
{
lean_object* x_1; lean_object* x_2; 
x_1 = l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__69;
x_2 = lean_alloc_ctor(3, 1, 0);
lean_ctor_set(x_2, 0, x_1);
return x_2;
}
}
static lean_object* _init_l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__71() {
_start:
{
lean_object* x_1; 
x_1 = lean_mk_string_unchecked("{ ", 2, 2);
return x_1;
}
}
static lean_object* _init_l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__72() {
_start:
{
lean_object* x_1; lean_object* x_2; 
x_1 = l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__71;
x_2 = lean_string_length(x_1);
return x_2;
}
}
static lean_object* _init_l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__73() {
_start:
{
lean_object* x_1; lean_object* x_2; 
x_1 = l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__72;
x_2 = lean_nat_to_int(x_1);
return x_2;
}
}
static lean_object* _init_l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__74() {
_start:
{
lean_object* x_1; lean_object* x_2; 
x_1 = l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__71;
x_2 = lean_alloc_ctor(3, 1, 0);
lean_ctor_set(x_2, 0, x_1);
return x_2;
}
}
static lean_object* _init_l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__75() {
_start:
{
lean_object* x_1; 
x_1 = lean_mk_string_unchecked(" }", 2, 2);
return x_1;
}
}
static lean_object* _init_l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__76() {
_start:
{
lean_object* x_1; lean_object* x_2; 
x_1 = l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__75;
x_2 = lean_alloc_ctor(3, 1, 0);
lean_ctor_set(x_2, 0, x_1);
return x_2;
}
}
static lean_object* _init_l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__77() {
_start:
{
lean_object* x_1; 
x_1 = lean_mk_string_unchecked("false", 5, 5);
return x_1;
}
}
static lean_object* _init_l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__78() {
_start:
{
lean_object* x_1; lean_object* x_2; 
x_1 = l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__77;
x_2 = lean_alloc_ctor(3, 1, 0);
lean_ctor_set(x_2, 0, x_1);
return x_2;
}
}
static lean_object* _init_l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__79() {
_start:
{
lean_object* x_1; lean_object* x_2; lean_object* x_3; 
x_1 = l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__33;
x_2 = l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__78;
x_3 = lean_alloc_ctor(4, 2, 0);
lean_ctor_set(x_3, 0, x_1);
lean_ctor_set(x_3, 1, x_2);
return x_3;
}
}
static lean_object* _init_l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__80() {
_start:
{
lean_object* x_1; uint8_t x_2; lean_object* x_3; 
x_1 = l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__79;
x_2 = 0;
x_3 = lean_alloc_ctor(6, 1, 1);
lean_ctor_set(x_3, 0, x_1);
lean_ctor_set_uint8(x_3, sizeof(void*)*1, x_2);
return x_3;
}
}
static lean_object* _init_l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__81() {
_start:
{
lean_object* x_1; 
x_1 = lean_mk_string_unchecked("true", 4, 4);
return x_1;
}
}
static lean_object* _init_l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__82() {
_start:
{
lean_object* x_1; lean_object* x_2; 
x_1 = l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__81;
x_2 = lean_alloc_ctor(3, 1, 0);
lean_ctor_set(x_2, 0, x_1);
return x_2;
}
}
static lean_object* _init_l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__83() {
_start:
{
lean_object* x_1; lean_object* x_2; lean_object* x_3; 
x_1 = l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__33;
x_2 = l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__82;
x_3 = lean_alloc_ctor(4, 2, 0);
lean_ctor_set(x_3, 0, x_1);
lean_ctor_set(x_3, 1, x_2);
return x_3;
}
}
static lean_object* _init_l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__84() {
_start:
{
lean_object* x_1; uint8_t x_2; lean_object* x_3; 
x_1 = l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__83;
x_2 = 0;
x_3 = lean_alloc_ctor(6, 1, 1);
lean_ctor_set(x_3, 0, x_1);
lean_ctor_set_uint8(x_3, sizeof(void*)*1, x_2);
return x_3;
}
}
LEAN_EXPORT lean_object* l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210_(lean_object* x_1, lean_object* x_2) {
_start:
{
lean_object* x_3; lean_object* x_4; lean_object* x_5; lean_object* x_6; lean_object* x_7; uint8_t x_8; lean_object* x_9; lean_object* x_10; lean_object* x_11; lean_object* x_12; lean_object* x_13; lean_object* x_14; lean_object* x_15; lean_object* x_16; lean_object* x_17; lean_object* x_18; lean_object* x_19; lean_object* x_20; lean_object* x_21; lean_object* x_22; lean_object* x_23; lean_object* x_24; lean_object* x_25; lean_object* x_26; lean_object* x_27; lean_object* x_28; lean_object* x_29; lean_object* x_30; lean_object* x_31; lean_object* x_32; lean_object* x_33; lean_object* x_34; lean_object* x_35; lean_object* x_36; lean_object* x_37; lean_object* x_38; lean_object* x_39; lean_object* x_40; lean_object* x_41; lean_object* x_42; lean_object* x_43; lean_object* x_44; lean_object* x_45; lean_object* x_46; lean_object* x_47; lean_object* x_48; lean_object* x_49; lean_object* x_50; lean_object* x_51; lean_object* x_52; lean_object* x_53; lean_object* x_54; lean_object* x_55; lean_object* x_56; lean_object* x_57; lean_object* x_58; lean_object* x_59; lean_object* x_60; lean_object* x_61; lean_object* x_62; lean_object* x_63; lean_object* x_64; lean_object* x_65; lean_object* x_66; lean_object* x_67; lean_object* x_68; lean_object* x_69; lean_object* x_70; lean_object* x_71; lean_object* x_72; lean_object* x_73; lean_object* x_74; lean_object* x_75; lean_object* x_76; lean_object* x_77; lean_object* x_78; lean_object* x_79; lean_object* x_80; lean_object* x_81; uint8_t x_82; lean_object* x_83; uint8_t x_84; lean_object* x_85; lean_object* x_86; lean_object* x_87; lean_object* x_88; lean_object* x_89; lean_object* x_90; lean_object* x_91; uint8_t x_92; lean_object* x_93; uint8_t x_94; lean_object* x_95; lean_object* x_96; lean_object* x_97; lean_object* x_98; lean_object* x_99; lean_object* x_100; lean_object* x_101; lean_object* x_102; lean_object* x_103; lean_object* x_104; lean_object* x_105; lean_object* x_106; uint8_t x_107; lean_object* x_108; lean_object* x_109; lean_object* x_110; lean_object* x_111; lean_object* x_112; lean_object* x_113; lean_object* x_114; uint8_t x_115; lean_object* x_116; lean_object* x_117; lean_object* x_118; lean_object* x_119; lean_object* x_120; lean_object* x_121; lean_object* x_122; lean_object* x_123; lean_object* x_124; lean_object* x_125; lean_object* x_126; lean_object* x_127; lean_object* x_128; lean_object* x_129; lean_object* x_130; lean_object* x_131; lean_object* x_132; lean_object* x_133; lean_object* x_134; lean_object* x_135; lean_object* x_136; lean_object* x_137; lean_object* x_138; lean_object* x_139; lean_object* x_140; lean_object* x_141; lean_object* x_142; lean_object* x_143; lean_object* x_144; uint8_t x_145; uint8_t x_146; uint8_t x_147; uint8_t x_148; lean_object* x_149; 
x_3 = lean_ctor_get(x_1, 0);
lean_inc(x_3);
x_4 = l___private_Init_Data_Repr_0__Nat_reprFast(x_3);
x_5 = lean_alloc_ctor(3, 1, 0);
lean_ctor_set(x_5, 0, x_4);
x_6 = l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__7;
x_7 = lean_alloc_ctor(4, 2, 0);
lean_ctor_set(x_7, 0, x_6);
lean_ctor_set(x_7, 1, x_5);
x_8 = 0;
x_9 = lean_alloc_ctor(6, 1, 1);
lean_ctor_set(x_9, 0, x_7);
lean_ctor_set_uint8(x_9, sizeof(void*)*1, x_8);
x_10 = l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__6;
x_11 = lean_alloc_ctor(5, 2, 0);
lean_ctor_set(x_11, 0, x_10);
lean_ctor_set(x_11, 1, x_9);
x_12 = l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__9;
x_13 = lean_alloc_ctor(5, 2, 0);
lean_ctor_set(x_13, 0, x_11);
lean_ctor_set(x_13, 1, x_12);
x_14 = lean_box(1);
x_15 = lean_alloc_ctor(5, 2, 0);
lean_ctor_set(x_15, 0, x_13);
lean_ctor_set(x_15, 1, x_14);
x_16 = l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__11;
x_17 = lean_alloc_ctor(5, 2, 0);
lean_ctor_set(x_17, 0, x_15);
lean_ctor_set(x_17, 1, x_16);
x_18 = l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__5;
x_19 = lean_alloc_ctor(5, 2, 0);
lean_ctor_set(x_19, 0, x_17);
lean_ctor_set(x_19, 1, x_18);
x_20 = lean_ctor_get(x_1, 1);
lean_inc(x_20);
x_21 = l___private_Init_Data_Repr_0__Nat_reprFast(x_20);
x_22 = lean_alloc_ctor(3, 1, 0);
lean_ctor_set(x_22, 0, x_21);
x_23 = l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__12;
x_24 = lean_alloc_ctor(4, 2, 0);
lean_ctor_set(x_24, 0, x_23);
lean_ctor_set(x_24, 1, x_22);
x_25 = lean_alloc_ctor(6, 1, 1);
lean_ctor_set(x_25, 0, x_24);
lean_ctor_set_uint8(x_25, sizeof(void*)*1, x_8);
x_26 = lean_alloc_ctor(5, 2, 0);
lean_ctor_set(x_26, 0, x_19);
lean_ctor_set(x_26, 1, x_25);
x_27 = lean_alloc_ctor(5, 2, 0);
lean_ctor_set(x_27, 0, x_26);
lean_ctor_set(x_27, 1, x_12);
x_28 = lean_alloc_ctor(5, 2, 0);
lean_ctor_set(x_28, 0, x_27);
lean_ctor_set(x_28, 1, x_14);
x_29 = l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__14;
x_30 = lean_alloc_ctor(5, 2, 0);
lean_ctor_set(x_30, 0, x_28);
lean_ctor_set(x_30, 1, x_29);
x_31 = lean_alloc_ctor(5, 2, 0);
lean_ctor_set(x_31, 0, x_30);
lean_ctor_set(x_31, 1, x_18);
x_32 = lean_ctor_get(x_1, 2);
lean_inc(x_32);
x_33 = l___private_Init_Data_Repr_0__Nat_reprFast(x_32);
x_34 = lean_alloc_ctor(3, 1, 0);
lean_ctor_set(x_34, 0, x_33);
x_35 = l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__15;
x_36 = lean_alloc_ctor(4, 2, 0);
lean_ctor_set(x_36, 0, x_35);
lean_ctor_set(x_36, 1, x_34);
x_37 = lean_alloc_ctor(6, 1, 1);
lean_ctor_set(x_37, 0, x_36);
lean_ctor_set_uint8(x_37, sizeof(void*)*1, x_8);
x_38 = lean_alloc_ctor(5, 2, 0);
lean_ctor_set(x_38, 0, x_31);
lean_ctor_set(x_38, 1, x_37);
x_39 = lean_alloc_ctor(5, 2, 0);
lean_ctor_set(x_39, 0, x_38);
lean_ctor_set(x_39, 1, x_12);
x_40 = lean_alloc_ctor(5, 2, 0);
lean_ctor_set(x_40, 0, x_39);
lean_ctor_set(x_40, 1, x_14);
x_41 = l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__17;
x_42 = lean_alloc_ctor(5, 2, 0);
lean_ctor_set(x_42, 0, x_40);
lean_ctor_set(x_42, 1, x_41);
x_43 = lean_alloc_ctor(5, 2, 0);
lean_ctor_set(x_43, 0, x_42);
lean_ctor_set(x_43, 1, x_18);
x_44 = lean_ctor_get(x_1, 3);
lean_inc(x_44);
x_45 = l___private_Init_Data_Repr_0__Nat_reprFast(x_44);
x_46 = lean_alloc_ctor(3, 1, 0);
lean_ctor_set(x_46, 0, x_45);
x_47 = l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__18;
x_48 = lean_alloc_ctor(4, 2, 0);
lean_ctor_set(x_48, 0, x_47);
lean_ctor_set(x_48, 1, x_46);
x_49 = lean_alloc_ctor(6, 1, 1);
lean_ctor_set(x_49, 0, x_48);
lean_ctor_set_uint8(x_49, sizeof(void*)*1, x_8);
x_50 = lean_alloc_ctor(5, 2, 0);
lean_ctor_set(x_50, 0, x_43);
lean_ctor_set(x_50, 1, x_49);
x_51 = lean_alloc_ctor(5, 2, 0);
lean_ctor_set(x_51, 0, x_50);
lean_ctor_set(x_51, 1, x_12);
x_52 = lean_alloc_ctor(5, 2, 0);
lean_ctor_set(x_52, 0, x_51);
lean_ctor_set(x_52, 1, x_14);
x_53 = l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__20;
x_54 = lean_alloc_ctor(5, 2, 0);
lean_ctor_set(x_54, 0, x_52);
lean_ctor_set(x_54, 1, x_53);
x_55 = lean_alloc_ctor(5, 2, 0);
lean_ctor_set(x_55, 0, x_54);
lean_ctor_set(x_55, 1, x_18);
x_56 = lean_ctor_get(x_1, 4);
lean_inc(x_56);
x_57 = l___private_Init_Data_Repr_0__Nat_reprFast(x_56);
x_58 = lean_alloc_ctor(3, 1, 0);
lean_ctor_set(x_58, 0, x_57);
x_59 = l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__21;
x_60 = lean_alloc_ctor(4, 2, 0);
lean_ctor_set(x_60, 0, x_59);
lean_ctor_set(x_60, 1, x_58);
x_61 = lean_alloc_ctor(6, 1, 1);
lean_ctor_set(x_61, 0, x_60);
lean_ctor_set_uint8(x_61, sizeof(void*)*1, x_8);
x_62 = lean_alloc_ctor(5, 2, 0);
lean_ctor_set(x_62, 0, x_55);
lean_ctor_set(x_62, 1, x_61);
x_63 = lean_alloc_ctor(5, 2, 0);
lean_ctor_set(x_63, 0, x_62);
lean_ctor_set(x_63, 1, x_12);
x_64 = lean_alloc_ctor(5, 2, 0);
lean_ctor_set(x_64, 0, x_63);
lean_ctor_set(x_64, 1, x_14);
x_65 = l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__23;
x_66 = lean_alloc_ctor(5, 2, 0);
lean_ctor_set(x_66, 0, x_64);
lean_ctor_set(x_66, 1, x_65);
x_67 = lean_alloc_ctor(5, 2, 0);
lean_ctor_set(x_67, 0, x_66);
lean_ctor_set(x_67, 1, x_18);
x_68 = lean_ctor_get(x_1, 5);
lean_inc(x_68);
x_69 = l___private_Init_Data_Repr_0__Nat_reprFast(x_68);
x_70 = lean_alloc_ctor(3, 1, 0);
lean_ctor_set(x_70, 0, x_69);
x_71 = l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__24;
x_72 = lean_alloc_ctor(4, 2, 0);
lean_ctor_set(x_72, 0, x_71);
lean_ctor_set(x_72, 1, x_70);
x_73 = lean_alloc_ctor(6, 1, 1);
lean_ctor_set(x_73, 0, x_72);
lean_ctor_set_uint8(x_73, sizeof(void*)*1, x_8);
x_74 = lean_alloc_ctor(5, 2, 0);
lean_ctor_set(x_74, 0, x_67);
lean_ctor_set(x_74, 1, x_73);
x_75 = lean_alloc_ctor(5, 2, 0);
lean_ctor_set(x_75, 0, x_74);
lean_ctor_set(x_75, 1, x_12);
x_76 = lean_alloc_ctor(5, 2, 0);
lean_ctor_set(x_76, 0, x_75);
lean_ctor_set(x_76, 1, x_14);
x_77 = l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__26;
x_78 = lean_alloc_ctor(5, 2, 0);
lean_ctor_set(x_78, 0, x_76);
lean_ctor_set(x_78, 1, x_77);
x_79 = lean_alloc_ctor(5, 2, 0);
lean_ctor_set(x_79, 0, x_78);
lean_ctor_set(x_79, 1, x_18);
x_80 = lean_ctor_get(x_1, 6);
lean_inc(x_80);
x_81 = l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__28;
x_82 = lean_int_dec_lt(x_80, x_81);
x_83 = lean_ctor_get(x_1, 7);
lean_inc(x_83);
x_84 = lean_int_dec_lt(x_83, x_81);
x_85 = lean_ctor_get(x_1, 8);
lean_inc(x_85);
x_86 = l___private_Init_Data_Repr_0__Nat_reprFast(x_85);
x_87 = lean_alloc_ctor(3, 1, 0);
lean_ctor_set(x_87, 0, x_86);
x_88 = l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__33;
x_89 = lean_alloc_ctor(4, 2, 0);
lean_ctor_set(x_89, 0, x_88);
lean_ctor_set(x_89, 1, x_87);
x_90 = lean_alloc_ctor(6, 1, 1);
lean_ctor_set(x_90, 0, x_89);
lean_ctor_set_uint8(x_90, sizeof(void*)*1, x_8);
x_91 = lean_ctor_get(x_1, 9);
lean_inc(x_91);
x_92 = lean_int_dec_lt(x_91, x_81);
x_93 = lean_ctor_get(x_1, 10);
lean_inc(x_93);
x_94 = lean_int_dec_lt(x_93, x_81);
x_95 = lean_ctor_get(x_1, 11);
lean_inc(x_95);
x_96 = l___private_Init_Data_Repr_0__Nat_reprFast(x_95);
x_97 = lean_alloc_ctor(3, 1, 0);
lean_ctor_set(x_97, 0, x_96);
x_98 = lean_alloc_ctor(4, 2, 0);
lean_ctor_set(x_98, 0, x_71);
lean_ctor_set(x_98, 1, x_97);
x_99 = lean_alloc_ctor(6, 1, 1);
lean_ctor_set(x_99, 0, x_98);
lean_ctor_set_uint8(x_99, sizeof(void*)*1, x_8);
x_100 = lean_ctor_get(x_1, 12);
lean_inc(x_100);
x_101 = l___private_Init_Data_Repr_0__Nat_reprFast(x_100);
x_102 = lean_alloc_ctor(3, 1, 0);
lean_ctor_set(x_102, 0, x_101);
x_103 = l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__43;
x_104 = lean_alloc_ctor(4, 2, 0);
lean_ctor_set(x_104, 0, x_103);
lean_ctor_set(x_104, 1, x_102);
x_105 = lean_alloc_ctor(6, 1, 1);
lean_ctor_set(x_105, 0, x_104);
lean_ctor_set_uint8(x_105, sizeof(void*)*1, x_8);
x_106 = lean_ctor_get(x_1, 13);
lean_inc(x_106);
x_107 = lean_int_dec_lt(x_106, x_81);
x_108 = lean_ctor_get(x_1, 14);
lean_inc(x_108);
x_109 = l___private_Init_Data_Repr_0__Nat_reprFast(x_108);
x_110 = lean_alloc_ctor(3, 1, 0);
lean_ctor_set(x_110, 0, x_109);
x_111 = l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__46;
x_112 = lean_alloc_ctor(4, 2, 0);
lean_ctor_set(x_112, 0, x_111);
lean_ctor_set(x_112, 1, x_110);
x_113 = lean_alloc_ctor(6, 1, 1);
lean_ctor_set(x_113, 0, x_112);
lean_ctor_set_uint8(x_113, sizeof(void*)*1, x_8);
x_114 = lean_ctor_get(x_1, 15);
lean_inc(x_114);
x_115 = lean_int_dec_lt(x_114, x_81);
x_116 = lean_ctor_get(x_1, 16);
lean_inc(x_116);
x_117 = l___private_Init_Data_Repr_0__Nat_reprFast(x_116);
x_118 = lean_alloc_ctor(3, 1, 0);
lean_ctor_set(x_118, 0, x_117);
x_119 = l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__27;
x_120 = lean_alloc_ctor(4, 2, 0);
lean_ctor_set(x_120, 0, x_119);
lean_ctor_set(x_120, 1, x_118);
x_121 = lean_alloc_ctor(6, 1, 1);
lean_ctor_set(x_121, 0, x_120);
lean_ctor_set_uint8(x_121, sizeof(void*)*1, x_8);
x_122 = lean_ctor_get(x_1, 17);
lean_inc(x_122);
x_123 = l___private_Init_Data_Repr_0__Nat_reprFast(x_122);
x_124 = lean_alloc_ctor(3, 1, 0);
lean_ctor_set(x_124, 0, x_123);
x_125 = lean_alloc_ctor(4, 2, 0);
lean_ctor_set(x_125, 0, x_88);
lean_ctor_set(x_125, 1, x_124);
x_126 = lean_alloc_ctor(6, 1, 1);
lean_ctor_set(x_126, 0, x_125);
lean_ctor_set_uint8(x_126, sizeof(void*)*1, x_8);
x_127 = lean_ctor_get(x_1, 18);
lean_inc(x_127);
x_128 = l___private_Init_Data_Repr_0__Nat_reprFast(x_127);
x_129 = lean_alloc_ctor(3, 1, 0);
lean_ctor_set(x_129, 0, x_128);
x_130 = l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__57;
x_131 = lean_alloc_ctor(4, 2, 0);
lean_ctor_set(x_131, 0, x_130);
lean_ctor_set(x_131, 1, x_129);
x_132 = lean_alloc_ctor(6, 1, 1);
lean_ctor_set(x_132, 0, x_131);
lean_ctor_set_uint8(x_132, sizeof(void*)*1, x_8);
x_133 = lean_ctor_get(x_1, 19);
lean_inc(x_133);
x_134 = l___private_Init_Data_Repr_0__Nat_reprFast(x_133);
x_135 = lean_alloc_ctor(3, 1, 0);
lean_ctor_set(x_135, 0, x_134);
x_136 = lean_alloc_ctor(4, 2, 0);
lean_ctor_set(x_136, 0, x_130);
lean_ctor_set(x_136, 1, x_135);
x_137 = lean_alloc_ctor(6, 1, 1);
lean_ctor_set(x_137, 0, x_136);
lean_ctor_set_uint8(x_137, sizeof(void*)*1, x_8);
x_138 = lean_ctor_get(x_1, 20);
lean_inc(x_138);
x_139 = l___private_Init_Data_Repr_0__Nat_reprFast(x_138);
x_140 = lean_alloc_ctor(3, 1, 0);
lean_ctor_set(x_140, 0, x_139);
x_141 = l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__62;
x_142 = lean_alloc_ctor(4, 2, 0);
lean_ctor_set(x_142, 0, x_141);
lean_ctor_set(x_142, 1, x_140);
x_143 = lean_alloc_ctor(6, 1, 1);
lean_ctor_set(x_143, 0, x_142);
lean_ctor_set_uint8(x_143, sizeof(void*)*1, x_8);
x_144 = lean_ctor_get(x_1, 21);
lean_inc(x_144);
x_145 = lean_int_dec_lt(x_144, x_81);
x_146 = lean_ctor_get_uint8(x_1, sizeof(void*)*22);
x_147 = lean_ctor_get_uint8(x_1, sizeof(void*)*22 + 1);
x_148 = lean_ctor_get_uint8(x_1, sizeof(void*)*22 + 2);
lean_dec(x_1);
if (x_82 == 0)
{
lean_object* x_352; lean_object* x_353; 
x_352 = l_Int_repr(x_80);
lean_dec(x_80);
x_353 = lean_alloc_ctor(3, 1, 0);
lean_ctor_set(x_353, 0, x_352);
x_149 = x_353;
goto block_351;
}
else
{
lean_object* x_354; lean_object* x_355; lean_object* x_356; lean_object* x_357; 
x_354 = l_Int_repr(x_80);
lean_dec(x_80);
x_355 = lean_alloc_ctor(3, 1, 0);
lean_ctor_set(x_355, 0, x_354);
x_356 = lean_unsigned_to_nat(0u);
x_357 = l_Repr_addAppParen(x_355, x_356);
x_149 = x_357;
goto block_351;
}
block_351:
{
lean_object* x_150; lean_object* x_151; lean_object* x_152; lean_object* x_153; lean_object* x_154; lean_object* x_155; lean_object* x_156; lean_object* x_157; lean_object* x_158; 
x_150 = lean_alloc_ctor(4, 2, 0);
lean_ctor_set(x_150, 0, x_119);
lean_ctor_set(x_150, 1, x_149);
x_151 = lean_alloc_ctor(6, 1, 1);
lean_ctor_set(x_151, 0, x_150);
lean_ctor_set_uint8(x_151, sizeof(void*)*1, x_8);
x_152 = lean_alloc_ctor(5, 2, 0);
lean_ctor_set(x_152, 0, x_79);
lean_ctor_set(x_152, 1, x_151);
x_153 = lean_alloc_ctor(5, 2, 0);
lean_ctor_set(x_153, 0, x_152);
lean_ctor_set(x_153, 1, x_12);
x_154 = lean_alloc_ctor(5, 2, 0);
lean_ctor_set(x_154, 0, x_153);
lean_ctor_set(x_154, 1, x_14);
x_155 = l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__30;
x_156 = lean_alloc_ctor(5, 2, 0);
lean_ctor_set(x_156, 0, x_154);
lean_ctor_set(x_156, 1, x_155);
x_157 = lean_alloc_ctor(5, 2, 0);
lean_ctor_set(x_157, 0, x_156);
lean_ctor_set(x_157, 1, x_18);
if (x_84 == 0)
{
lean_object* x_345; lean_object* x_346; 
x_345 = l_Int_repr(x_83);
lean_dec(x_83);
x_346 = lean_alloc_ctor(3, 1, 0);
lean_ctor_set(x_346, 0, x_345);
x_158 = x_346;
goto block_344;
}
else
{
lean_object* x_347; lean_object* x_348; lean_object* x_349; lean_object* x_350; 
x_347 = l_Int_repr(x_83);
lean_dec(x_83);
x_348 = lean_alloc_ctor(3, 1, 0);
lean_ctor_set(x_348, 0, x_347);
x_349 = lean_unsigned_to_nat(0u);
x_350 = l_Repr_addAppParen(x_348, x_349);
x_158 = x_350;
goto block_344;
}
block_344:
{
lean_object* x_159; lean_object* x_160; lean_object* x_161; lean_object* x_162; lean_object* x_163; lean_object* x_164; lean_object* x_165; lean_object* x_166; lean_object* x_167; lean_object* x_168; lean_object* x_169; lean_object* x_170; lean_object* x_171; lean_object* x_172; lean_object* x_173; 
x_159 = lean_alloc_ctor(4, 2, 0);
lean_ctor_set(x_159, 0, x_71);
lean_ctor_set(x_159, 1, x_158);
x_160 = lean_alloc_ctor(6, 1, 1);
lean_ctor_set(x_160, 0, x_159);
lean_ctor_set_uint8(x_160, sizeof(void*)*1, x_8);
x_161 = lean_alloc_ctor(5, 2, 0);
lean_ctor_set(x_161, 0, x_157);
lean_ctor_set(x_161, 1, x_160);
x_162 = lean_alloc_ctor(5, 2, 0);
lean_ctor_set(x_162, 0, x_161);
lean_ctor_set(x_162, 1, x_12);
x_163 = lean_alloc_ctor(5, 2, 0);
lean_ctor_set(x_163, 0, x_162);
lean_ctor_set(x_163, 1, x_14);
x_164 = l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__32;
x_165 = lean_alloc_ctor(5, 2, 0);
lean_ctor_set(x_165, 0, x_163);
lean_ctor_set(x_165, 1, x_164);
x_166 = lean_alloc_ctor(5, 2, 0);
lean_ctor_set(x_166, 0, x_165);
lean_ctor_set(x_166, 1, x_18);
x_167 = lean_alloc_ctor(5, 2, 0);
lean_ctor_set(x_167, 0, x_166);
lean_ctor_set(x_167, 1, x_90);
x_168 = lean_alloc_ctor(5, 2, 0);
lean_ctor_set(x_168, 0, x_167);
lean_ctor_set(x_168, 1, x_12);
x_169 = lean_alloc_ctor(5, 2, 0);
lean_ctor_set(x_169, 0, x_168);
lean_ctor_set(x_169, 1, x_14);
x_170 = l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__35;
x_171 = lean_alloc_ctor(5, 2, 0);
lean_ctor_set(x_171, 0, x_169);
lean_ctor_set(x_171, 1, x_170);
x_172 = lean_alloc_ctor(5, 2, 0);
lean_ctor_set(x_172, 0, x_171);
lean_ctor_set(x_172, 1, x_18);
if (x_92 == 0)
{
lean_object* x_338; lean_object* x_339; 
x_338 = l_Int_repr(x_91);
lean_dec(x_91);
x_339 = lean_alloc_ctor(3, 1, 0);
lean_ctor_set(x_339, 0, x_338);
x_173 = x_339;
goto block_337;
}
else
{
lean_object* x_340; lean_object* x_341; lean_object* x_342; lean_object* x_343; 
x_340 = l_Int_repr(x_91);
lean_dec(x_91);
x_341 = lean_alloc_ctor(3, 1, 0);
lean_ctor_set(x_341, 0, x_340);
x_342 = lean_unsigned_to_nat(0u);
x_343 = l_Repr_addAppParen(x_341, x_342);
x_173 = x_343;
goto block_337;
}
block_337:
{
lean_object* x_174; lean_object* x_175; lean_object* x_176; lean_object* x_177; lean_object* x_178; lean_object* x_179; lean_object* x_180; lean_object* x_181; lean_object* x_182; lean_object* x_183; 
x_174 = l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__36;
x_175 = lean_alloc_ctor(4, 2, 0);
lean_ctor_set(x_175, 0, x_174);
lean_ctor_set(x_175, 1, x_173);
x_176 = lean_alloc_ctor(6, 1, 1);
lean_ctor_set(x_176, 0, x_175);
lean_ctor_set_uint8(x_176, sizeof(void*)*1, x_8);
x_177 = lean_alloc_ctor(5, 2, 0);
lean_ctor_set(x_177, 0, x_172);
lean_ctor_set(x_177, 1, x_176);
x_178 = lean_alloc_ctor(5, 2, 0);
lean_ctor_set(x_178, 0, x_177);
lean_ctor_set(x_178, 1, x_12);
x_179 = lean_alloc_ctor(5, 2, 0);
lean_ctor_set(x_179, 0, x_178);
lean_ctor_set(x_179, 1, x_14);
x_180 = l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__38;
x_181 = lean_alloc_ctor(5, 2, 0);
lean_ctor_set(x_181, 0, x_179);
lean_ctor_set(x_181, 1, x_180);
x_182 = lean_alloc_ctor(5, 2, 0);
lean_ctor_set(x_182, 0, x_181);
lean_ctor_set(x_182, 1, x_18);
if (x_94 == 0)
{
lean_object* x_331; lean_object* x_332; 
x_331 = l_Int_repr(x_93);
lean_dec(x_93);
x_332 = lean_alloc_ctor(3, 1, 0);
lean_ctor_set(x_332, 0, x_331);
x_183 = x_332;
goto block_330;
}
else
{
lean_object* x_333; lean_object* x_334; lean_object* x_335; lean_object* x_336; 
x_333 = l_Int_repr(x_93);
lean_dec(x_93);
x_334 = lean_alloc_ctor(3, 1, 0);
lean_ctor_set(x_334, 0, x_333);
x_335 = lean_unsigned_to_nat(0u);
x_336 = l_Repr_addAppParen(x_334, x_335);
x_183 = x_336;
goto block_330;
}
block_330:
{
lean_object* x_184; lean_object* x_185; lean_object* x_186; lean_object* x_187; lean_object* x_188; lean_object* x_189; lean_object* x_190; lean_object* x_191; lean_object* x_192; lean_object* x_193; lean_object* x_194; lean_object* x_195; lean_object* x_196; lean_object* x_197; lean_object* x_198; lean_object* x_199; lean_object* x_200; lean_object* x_201; lean_object* x_202; lean_object* x_203; lean_object* x_204; 
x_184 = lean_alloc_ctor(4, 2, 0);
lean_ctor_set(x_184, 0, x_174);
lean_ctor_set(x_184, 1, x_183);
x_185 = lean_alloc_ctor(6, 1, 1);
lean_ctor_set(x_185, 0, x_184);
lean_ctor_set_uint8(x_185, sizeof(void*)*1, x_8);
x_186 = lean_alloc_ctor(5, 2, 0);
lean_ctor_set(x_186, 0, x_182);
lean_ctor_set(x_186, 1, x_185);
x_187 = lean_alloc_ctor(5, 2, 0);
lean_ctor_set(x_187, 0, x_186);
lean_ctor_set(x_187, 1, x_12);
x_188 = lean_alloc_ctor(5, 2, 0);
lean_ctor_set(x_188, 0, x_187);
lean_ctor_set(x_188, 1, x_14);
x_189 = l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__40;
x_190 = lean_alloc_ctor(5, 2, 0);
lean_ctor_set(x_190, 0, x_188);
lean_ctor_set(x_190, 1, x_189);
x_191 = lean_alloc_ctor(5, 2, 0);
lean_ctor_set(x_191, 0, x_190);
lean_ctor_set(x_191, 1, x_18);
x_192 = lean_alloc_ctor(5, 2, 0);
lean_ctor_set(x_192, 0, x_191);
lean_ctor_set(x_192, 1, x_99);
x_193 = lean_alloc_ctor(5, 2, 0);
lean_ctor_set(x_193, 0, x_192);
lean_ctor_set(x_193, 1, x_12);
x_194 = lean_alloc_ctor(5, 2, 0);
lean_ctor_set(x_194, 0, x_193);
lean_ctor_set(x_194, 1, x_14);
x_195 = l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__42;
x_196 = lean_alloc_ctor(5, 2, 0);
lean_ctor_set(x_196, 0, x_194);
lean_ctor_set(x_196, 1, x_195);
x_197 = lean_alloc_ctor(5, 2, 0);
lean_ctor_set(x_197, 0, x_196);
lean_ctor_set(x_197, 1, x_18);
x_198 = lean_alloc_ctor(5, 2, 0);
lean_ctor_set(x_198, 0, x_197);
lean_ctor_set(x_198, 1, x_105);
x_199 = lean_alloc_ctor(5, 2, 0);
lean_ctor_set(x_199, 0, x_198);
lean_ctor_set(x_199, 1, x_12);
x_200 = lean_alloc_ctor(5, 2, 0);
lean_ctor_set(x_200, 0, x_199);
lean_ctor_set(x_200, 1, x_14);
x_201 = l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__45;
x_202 = lean_alloc_ctor(5, 2, 0);
lean_ctor_set(x_202, 0, x_200);
lean_ctor_set(x_202, 1, x_201);
x_203 = lean_alloc_ctor(5, 2, 0);
lean_ctor_set(x_203, 0, x_202);
lean_ctor_set(x_203, 1, x_18);
if (x_107 == 0)
{
lean_object* x_324; lean_object* x_325; 
x_324 = l_Int_repr(x_106);
lean_dec(x_106);
x_325 = lean_alloc_ctor(3, 1, 0);
lean_ctor_set(x_325, 0, x_324);
x_204 = x_325;
goto block_323;
}
else
{
lean_object* x_326; lean_object* x_327; lean_object* x_328; lean_object* x_329; 
x_326 = l_Int_repr(x_106);
lean_dec(x_106);
x_327 = lean_alloc_ctor(3, 1, 0);
lean_ctor_set(x_327, 0, x_326);
x_328 = lean_unsigned_to_nat(0u);
x_329 = l_Repr_addAppParen(x_327, x_328);
x_204 = x_329;
goto block_323;
}
block_323:
{
lean_object* x_205; lean_object* x_206; lean_object* x_207; lean_object* x_208; lean_object* x_209; lean_object* x_210; lean_object* x_211; lean_object* x_212; lean_object* x_213; lean_object* x_214; lean_object* x_215; lean_object* x_216; lean_object* x_217; lean_object* x_218; lean_object* x_219; 
x_205 = lean_alloc_ctor(4, 2, 0);
lean_ctor_set(x_205, 0, x_111);
lean_ctor_set(x_205, 1, x_204);
x_206 = lean_alloc_ctor(6, 1, 1);
lean_ctor_set(x_206, 0, x_205);
lean_ctor_set_uint8(x_206, sizeof(void*)*1, x_8);
x_207 = lean_alloc_ctor(5, 2, 0);
lean_ctor_set(x_207, 0, x_203);
lean_ctor_set(x_207, 1, x_206);
x_208 = lean_alloc_ctor(5, 2, 0);
lean_ctor_set(x_208, 0, x_207);
lean_ctor_set(x_208, 1, x_12);
x_209 = lean_alloc_ctor(5, 2, 0);
lean_ctor_set(x_209, 0, x_208);
lean_ctor_set(x_209, 1, x_14);
x_210 = l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__48;
x_211 = lean_alloc_ctor(5, 2, 0);
lean_ctor_set(x_211, 0, x_209);
lean_ctor_set(x_211, 1, x_210);
x_212 = lean_alloc_ctor(5, 2, 0);
lean_ctor_set(x_212, 0, x_211);
lean_ctor_set(x_212, 1, x_18);
x_213 = lean_alloc_ctor(5, 2, 0);
lean_ctor_set(x_213, 0, x_212);
lean_ctor_set(x_213, 1, x_113);
x_214 = lean_alloc_ctor(5, 2, 0);
lean_ctor_set(x_214, 0, x_213);
lean_ctor_set(x_214, 1, x_12);
x_215 = lean_alloc_ctor(5, 2, 0);
lean_ctor_set(x_215, 0, x_214);
lean_ctor_set(x_215, 1, x_14);
x_216 = l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__50;
x_217 = lean_alloc_ctor(5, 2, 0);
lean_ctor_set(x_217, 0, x_215);
lean_ctor_set(x_217, 1, x_216);
x_218 = lean_alloc_ctor(5, 2, 0);
lean_ctor_set(x_218, 0, x_217);
lean_ctor_set(x_218, 1, x_18);
if (x_115 == 0)
{
lean_object* x_317; lean_object* x_318; 
x_317 = l_Int_repr(x_114);
lean_dec(x_114);
x_318 = lean_alloc_ctor(3, 1, 0);
lean_ctor_set(x_318, 0, x_317);
x_219 = x_318;
goto block_316;
}
else
{
lean_object* x_319; lean_object* x_320; lean_object* x_321; lean_object* x_322; 
x_319 = l_Int_repr(x_114);
lean_dec(x_114);
x_320 = lean_alloc_ctor(3, 1, 0);
lean_ctor_set(x_320, 0, x_319);
x_321 = lean_unsigned_to_nat(0u);
x_322 = l_Repr_addAppParen(x_320, x_321);
x_219 = x_322;
goto block_316;
}
block_316:
{
lean_object* x_220; lean_object* x_221; lean_object* x_222; lean_object* x_223; lean_object* x_224; lean_object* x_225; lean_object* x_226; lean_object* x_227; lean_object* x_228; lean_object* x_229; lean_object* x_230; lean_object* x_231; lean_object* x_232; lean_object* x_233; lean_object* x_234; lean_object* x_235; lean_object* x_236; lean_object* x_237; lean_object* x_238; lean_object* x_239; lean_object* x_240; lean_object* x_241; lean_object* x_242; lean_object* x_243; lean_object* x_244; lean_object* x_245; lean_object* x_246; lean_object* x_247; lean_object* x_248; lean_object* x_249; lean_object* x_250; lean_object* x_251; lean_object* x_252; lean_object* x_253; lean_object* x_254; lean_object* x_255; lean_object* x_256; lean_object* x_257; lean_object* x_258; 
x_220 = lean_alloc_ctor(4, 2, 0);
lean_ctor_set(x_220, 0, x_119);
lean_ctor_set(x_220, 1, x_219);
x_221 = lean_alloc_ctor(6, 1, 1);
lean_ctor_set(x_221, 0, x_220);
lean_ctor_set_uint8(x_221, sizeof(void*)*1, x_8);
x_222 = lean_alloc_ctor(5, 2, 0);
lean_ctor_set(x_222, 0, x_218);
lean_ctor_set(x_222, 1, x_221);
x_223 = lean_alloc_ctor(5, 2, 0);
lean_ctor_set(x_223, 0, x_222);
lean_ctor_set(x_223, 1, x_12);
x_224 = lean_alloc_ctor(5, 2, 0);
lean_ctor_set(x_224, 0, x_223);
lean_ctor_set(x_224, 1, x_14);
x_225 = l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__52;
x_226 = lean_alloc_ctor(5, 2, 0);
lean_ctor_set(x_226, 0, x_224);
lean_ctor_set(x_226, 1, x_225);
x_227 = lean_alloc_ctor(5, 2, 0);
lean_ctor_set(x_227, 0, x_226);
lean_ctor_set(x_227, 1, x_18);
x_228 = lean_alloc_ctor(5, 2, 0);
lean_ctor_set(x_228, 0, x_227);
lean_ctor_set(x_228, 1, x_121);
x_229 = lean_alloc_ctor(5, 2, 0);
lean_ctor_set(x_229, 0, x_228);
lean_ctor_set(x_229, 1, x_12);
x_230 = lean_alloc_ctor(5, 2, 0);
lean_ctor_set(x_230, 0, x_229);
lean_ctor_set(x_230, 1, x_14);
x_231 = l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__54;
x_232 = lean_alloc_ctor(5, 2, 0);
lean_ctor_set(x_232, 0, x_230);
lean_ctor_set(x_232, 1, x_231);
x_233 = lean_alloc_ctor(5, 2, 0);
lean_ctor_set(x_233, 0, x_232);
lean_ctor_set(x_233, 1, x_18);
x_234 = lean_alloc_ctor(5, 2, 0);
lean_ctor_set(x_234, 0, x_233);
lean_ctor_set(x_234, 1, x_126);
x_235 = lean_alloc_ctor(5, 2, 0);
lean_ctor_set(x_235, 0, x_234);
lean_ctor_set(x_235, 1, x_12);
x_236 = lean_alloc_ctor(5, 2, 0);
lean_ctor_set(x_236, 0, x_235);
lean_ctor_set(x_236, 1, x_14);
x_237 = l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__56;
x_238 = lean_alloc_ctor(5, 2, 0);
lean_ctor_set(x_238, 0, x_236);
lean_ctor_set(x_238, 1, x_237);
x_239 = lean_alloc_ctor(5, 2, 0);
lean_ctor_set(x_239, 0, x_238);
lean_ctor_set(x_239, 1, x_18);
x_240 = lean_alloc_ctor(5, 2, 0);
lean_ctor_set(x_240, 0, x_239);
lean_ctor_set(x_240, 1, x_132);
x_241 = lean_alloc_ctor(5, 2, 0);
lean_ctor_set(x_241, 0, x_240);
lean_ctor_set(x_241, 1, x_12);
x_242 = lean_alloc_ctor(5, 2, 0);
lean_ctor_set(x_242, 0, x_241);
lean_ctor_set(x_242, 1, x_14);
x_243 = l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__59;
x_244 = lean_alloc_ctor(5, 2, 0);
lean_ctor_set(x_244, 0, x_242);
lean_ctor_set(x_244, 1, x_243);
x_245 = lean_alloc_ctor(5, 2, 0);
lean_ctor_set(x_245, 0, x_244);
lean_ctor_set(x_245, 1, x_18);
x_246 = lean_alloc_ctor(5, 2, 0);
lean_ctor_set(x_246, 0, x_245);
lean_ctor_set(x_246, 1, x_137);
x_247 = lean_alloc_ctor(5, 2, 0);
lean_ctor_set(x_247, 0, x_246);
lean_ctor_set(x_247, 1, x_12);
x_248 = lean_alloc_ctor(5, 2, 0);
lean_ctor_set(x_248, 0, x_247);
lean_ctor_set(x_248, 1, x_14);
x_249 = l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__61;
x_250 = lean_alloc_ctor(5, 2, 0);
lean_ctor_set(x_250, 0, x_248);
lean_ctor_set(x_250, 1, x_249);
x_251 = lean_alloc_ctor(5, 2, 0);
lean_ctor_set(x_251, 0, x_250);
lean_ctor_set(x_251, 1, x_18);
x_252 = lean_alloc_ctor(5, 2, 0);
lean_ctor_set(x_252, 0, x_251);
lean_ctor_set(x_252, 1, x_143);
x_253 = lean_alloc_ctor(5, 2, 0);
lean_ctor_set(x_253, 0, x_252);
lean_ctor_set(x_253, 1, x_12);
x_254 = lean_alloc_ctor(5, 2, 0);
lean_ctor_set(x_254, 0, x_253);
lean_ctor_set(x_254, 1, x_14);
x_255 = l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__64;
x_256 = lean_alloc_ctor(5, 2, 0);
lean_ctor_set(x_256, 0, x_254);
lean_ctor_set(x_256, 1, x_255);
x_257 = lean_alloc_ctor(5, 2, 0);
lean_ctor_set(x_257, 0, x_256);
lean_ctor_set(x_257, 1, x_18);
if (x_145 == 0)
{
lean_object* x_310; lean_object* x_311; 
x_310 = l_Int_repr(x_144);
lean_dec(x_144);
x_311 = lean_alloc_ctor(3, 1, 0);
lean_ctor_set(x_311, 0, x_310);
x_258 = x_311;
goto block_309;
}
else
{
lean_object* x_312; lean_object* x_313; lean_object* x_314; lean_object* x_315; 
x_312 = l_Int_repr(x_144);
lean_dec(x_144);
x_313 = lean_alloc_ctor(3, 1, 0);
lean_ctor_set(x_313, 0, x_312);
x_314 = lean_unsigned_to_nat(0u);
x_315 = l_Repr_addAppParen(x_313, x_314);
x_258 = x_315;
goto block_309;
}
block_309:
{
lean_object* x_259; lean_object* x_260; lean_object* x_261; lean_object* x_262; lean_object* x_263; lean_object* x_264; lean_object* x_265; lean_object* x_266; lean_object* x_267; 
x_259 = lean_alloc_ctor(4, 2, 0);
lean_ctor_set(x_259, 0, x_35);
lean_ctor_set(x_259, 1, x_258);
x_260 = lean_alloc_ctor(6, 1, 1);
lean_ctor_set(x_260, 0, x_259);
lean_ctor_set_uint8(x_260, sizeof(void*)*1, x_8);
x_261 = lean_alloc_ctor(5, 2, 0);
lean_ctor_set(x_261, 0, x_257);
lean_ctor_set(x_261, 1, x_260);
x_262 = lean_alloc_ctor(5, 2, 0);
lean_ctor_set(x_262, 0, x_261);
lean_ctor_set(x_262, 1, x_12);
x_263 = lean_alloc_ctor(5, 2, 0);
lean_ctor_set(x_263, 0, x_262);
lean_ctor_set(x_263, 1, x_14);
x_264 = l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__66;
x_265 = lean_alloc_ctor(5, 2, 0);
lean_ctor_set(x_265, 0, x_263);
lean_ctor_set(x_265, 1, x_264);
x_266 = lean_alloc_ctor(5, 2, 0);
lean_ctor_set(x_266, 0, x_265);
lean_ctor_set(x_266, 1, x_18);
if (x_146 == 0)
{
lean_object* x_307; 
x_307 = l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__78;
x_267 = x_307;
goto block_306;
}
else
{
lean_object* x_308; 
x_308 = l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__82;
x_267 = x_308;
goto block_306;
}
block_306:
{
lean_object* x_268; lean_object* x_269; lean_object* x_270; lean_object* x_271; lean_object* x_272; lean_object* x_273; lean_object* x_274; lean_object* x_275; lean_object* x_276; 
x_268 = lean_alloc_ctor(4, 2, 0);
lean_ctor_set(x_268, 0, x_174);
lean_ctor_set(x_268, 1, x_267);
x_269 = lean_alloc_ctor(6, 1, 1);
lean_ctor_set(x_269, 0, x_268);
lean_ctor_set_uint8(x_269, sizeof(void*)*1, x_8);
x_270 = lean_alloc_ctor(5, 2, 0);
lean_ctor_set(x_270, 0, x_266);
lean_ctor_set(x_270, 1, x_269);
x_271 = lean_alloc_ctor(5, 2, 0);
lean_ctor_set(x_271, 0, x_270);
lean_ctor_set(x_271, 1, x_12);
x_272 = lean_alloc_ctor(5, 2, 0);
lean_ctor_set(x_272, 0, x_271);
lean_ctor_set(x_272, 1, x_14);
x_273 = l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__68;
x_274 = lean_alloc_ctor(5, 2, 0);
lean_ctor_set(x_274, 0, x_272);
lean_ctor_set(x_274, 1, x_273);
x_275 = lean_alloc_ctor(5, 2, 0);
lean_ctor_set(x_275, 0, x_274);
lean_ctor_set(x_275, 1, x_18);
if (x_147 == 0)
{
lean_object* x_304; 
x_304 = l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__78;
x_276 = x_304;
goto block_303;
}
else
{
lean_object* x_305; 
x_305 = l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__82;
x_276 = x_305;
goto block_303;
}
block_303:
{
lean_object* x_277; lean_object* x_278; lean_object* x_279; lean_object* x_280; lean_object* x_281; lean_object* x_282; lean_object* x_283; lean_object* x_284; 
x_277 = lean_alloc_ctor(4, 2, 0);
lean_ctor_set(x_277, 0, x_174);
lean_ctor_set(x_277, 1, x_276);
x_278 = lean_alloc_ctor(6, 1, 1);
lean_ctor_set(x_278, 0, x_277);
lean_ctor_set_uint8(x_278, sizeof(void*)*1, x_8);
x_279 = lean_alloc_ctor(5, 2, 0);
lean_ctor_set(x_279, 0, x_275);
lean_ctor_set(x_279, 1, x_278);
x_280 = lean_alloc_ctor(5, 2, 0);
lean_ctor_set(x_280, 0, x_279);
lean_ctor_set(x_280, 1, x_12);
x_281 = lean_alloc_ctor(5, 2, 0);
lean_ctor_set(x_281, 0, x_280);
lean_ctor_set(x_281, 1, x_14);
x_282 = l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__70;
x_283 = lean_alloc_ctor(5, 2, 0);
lean_ctor_set(x_283, 0, x_281);
lean_ctor_set(x_283, 1, x_282);
x_284 = lean_alloc_ctor(5, 2, 0);
lean_ctor_set(x_284, 0, x_283);
lean_ctor_set(x_284, 1, x_18);
if (x_148 == 0)
{
lean_object* x_285; lean_object* x_286; lean_object* x_287; lean_object* x_288; lean_object* x_289; lean_object* x_290; lean_object* x_291; lean_object* x_292; lean_object* x_293; 
x_285 = l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__80;
x_286 = lean_alloc_ctor(5, 2, 0);
lean_ctor_set(x_286, 0, x_284);
lean_ctor_set(x_286, 1, x_285);
x_287 = l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__74;
x_288 = lean_alloc_ctor(5, 2, 0);
lean_ctor_set(x_288, 0, x_287);
lean_ctor_set(x_288, 1, x_286);
x_289 = l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__76;
x_290 = lean_alloc_ctor(5, 2, 0);
lean_ctor_set(x_290, 0, x_288);
lean_ctor_set(x_290, 1, x_289);
x_291 = l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__73;
x_292 = lean_alloc_ctor(4, 2, 0);
lean_ctor_set(x_292, 0, x_291);
lean_ctor_set(x_292, 1, x_290);
x_293 = lean_alloc_ctor(6, 1, 1);
lean_ctor_set(x_293, 0, x_292);
lean_ctor_set_uint8(x_293, sizeof(void*)*1, x_8);
return x_293;
}
else
{
lean_object* x_294; lean_object* x_295; lean_object* x_296; lean_object* x_297; lean_object* x_298; lean_object* x_299; lean_object* x_300; lean_object* x_301; lean_object* x_302; 
x_294 = l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__84;
x_295 = lean_alloc_ctor(5, 2, 0);
lean_ctor_set(x_295, 0, x_284);
lean_ctor_set(x_295, 1, x_294);
x_296 = l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__74;
x_297 = lean_alloc_ctor(5, 2, 0);
lean_ctor_set(x_297, 0, x_296);
lean_ctor_set(x_297, 1, x_295);
x_298 = l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__76;
x_299 = lean_alloc_ctor(5, 2, 0);
lean_ctor_set(x_299, 0, x_297);
lean_ctor_set(x_299, 1, x_298);
x_300 = l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__73;
x_301 = lean_alloc_ctor(4, 2, 0);
lean_ctor_set(x_301, 0, x_300);
lean_ctor_set(x_301, 1, x_299);
x_302 = lean_alloc_ctor(6, 1, 1);
lean_ctor_set(x_302, 0, x_301);
lean_ctor_set_uint8(x_302, sizeof(void*)*1, x_8);
return x_302;
}
}
}
}
}
}
}
}
}
}
}
}
LEAN_EXPORT lean_object* l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____boxed(lean_object* x_1, lean_object* x_2) {
_start:
{
lean_object* x_3; 
x_3 = l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210_(x_1, x_2);
lean_dec(x_2);
return x_3;
}
}
static lean_object* _init_l_MangoV4_instReprPerpMarket___closed__1() {
_start:
{
lean_object* x_1; 
x_1 = lean_alloc_closure((void*)(l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____boxed), 2, 0);
return x_1;
}
}
static lean_object* _init_l_MangoV4_instReprPerpMarket() {
_start:
{
lean_object* x_1; 
x_1 = l_MangoV4_instReprPerpMarket___closed__1;
return x_1;
}
}
static lean_object* _init_l___private_MangoState_PerpMarket_0__MangoV4_reprPerpPosition____x40_MangoState_PerpMarket___hyg_829____closed__1() {
_start:
{
lean_object* x_1; 
x_1 = lean_mk_string_unchecked("market_index", 12, 12);
return x_1;
}
}
static lean_object* _init_l___private_MangoState_PerpMarket_0__MangoV4_reprPerpPosition____x40_MangoState_PerpMarket___hyg_829____closed__2() {
_start:
{
lean_object* x_1; lean_object* x_2; 
x_1 = l___private_MangoState_PerpMarket_0__MangoV4_reprPerpPosition____x40_MangoState_PerpMarket___hyg_829____closed__1;
x_2 = lean_alloc_ctor(3, 1, 0);
lean_ctor_set(x_2, 0, x_1);
return x_2;
}
}
static lean_object* _init_l___private_MangoState_PerpMarket_0__MangoV4_reprPerpPosition____x40_MangoState_PerpMarket___hyg_829____closed__3() {
_start:
{
lean_object* x_1; lean_object* x_2; lean_object* x_3; 
x_1 = lean_box(0);
x_2 = l___private_MangoState_PerpMarket_0__MangoV4_reprPerpPosition____x40_MangoState_PerpMarket___hyg_829____closed__2;
x_3 = lean_alloc_ctor(5, 2, 0);
lean_ctor_set(x_3, 0, x_1);
lean_ctor_set(x_3, 1, x_2);
return x_3;
}
}
static lean_object* _init_l___private_MangoState_PerpMarket_0__MangoV4_reprPerpPosition____x40_MangoState_PerpMarket___hyg_829____closed__4() {
_start:
{
lean_object* x_1; lean_object* x_2; lean_object* x_3; 
x_1 = l___private_MangoState_PerpMarket_0__MangoV4_reprPerpPosition____x40_MangoState_PerpMarket___hyg_829____closed__3;
x_2 = l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__5;
x_3 = lean_alloc_ctor(5, 2, 0);
lean_ctor_set(x_3, 0, x_1);
lean_ctor_set(x_3, 1, x_2);
return x_3;
}
}
static lean_object* _init_l___private_MangoState_PerpMarket_0__MangoV4_reprPerpPosition____x40_MangoState_PerpMarket___hyg_829____closed__5() {
_start:
{
lean_object* x_1; 
x_1 = lean_mk_string_unchecked("base_position_lots", 18, 18);
return x_1;
}
}
static lean_object* _init_l___private_MangoState_PerpMarket_0__MangoV4_reprPerpPosition____x40_MangoState_PerpMarket___hyg_829____closed__6() {
_start:
{
lean_object* x_1; lean_object* x_2; 
x_1 = l___private_MangoState_PerpMarket_0__MangoV4_reprPerpPosition____x40_MangoState_PerpMarket___hyg_829____closed__5;
x_2 = lean_alloc_ctor(3, 1, 0);
lean_ctor_set(x_2, 0, x_1);
return x_2;
}
}
static lean_object* _init_l___private_MangoState_PerpMarket_0__MangoV4_reprPerpPosition____x40_MangoState_PerpMarket___hyg_829____closed__7() {
_start:
{
lean_object* x_1; 
x_1 = lean_mk_string_unchecked("quote_position_native", 21, 21);
return x_1;
}
}
static lean_object* _init_l___private_MangoState_PerpMarket_0__MangoV4_reprPerpPosition____x40_MangoState_PerpMarket___hyg_829____closed__8() {
_start:
{
lean_object* x_1; lean_object* x_2; 
x_1 = l___private_MangoState_PerpMarket_0__MangoV4_reprPerpPosition____x40_MangoState_PerpMarket___hyg_829____closed__7;
x_2 = lean_alloc_ctor(3, 1, 0);
lean_ctor_set(x_2, 0, x_1);
return x_2;
}
}
static lean_object* _init_l___private_MangoState_PerpMarket_0__MangoV4_reprPerpPosition____x40_MangoState_PerpMarket___hyg_829____closed__9() {
_start:
{
lean_object* x_1; lean_object* x_2; 
x_1 = lean_unsigned_to_nat(25u);
x_2 = lean_nat_to_int(x_1);
return x_2;
}
}
static lean_object* _init_l___private_MangoState_PerpMarket_0__MangoV4_reprPerpPosition____x40_MangoState_PerpMarket___hyg_829____closed__10() {
_start:
{
lean_object* x_1; 
x_1 = lean_mk_string_unchecked("long_settled_funding", 20, 20);
return x_1;
}
}
static lean_object* _init_l___private_MangoState_PerpMarket_0__MangoV4_reprPerpPosition____x40_MangoState_PerpMarket___hyg_829____closed__11() {
_start:
{
lean_object* x_1; lean_object* x_2; 
x_1 = l___private_MangoState_PerpMarket_0__MangoV4_reprPerpPosition____x40_MangoState_PerpMarket___hyg_829____closed__10;
x_2 = lean_alloc_ctor(3, 1, 0);
lean_ctor_set(x_2, 0, x_1);
return x_2;
}
}
static lean_object* _init_l___private_MangoState_PerpMarket_0__MangoV4_reprPerpPosition____x40_MangoState_PerpMarket___hyg_829____closed__12() {
_start:
{
lean_object* x_1; 
x_1 = lean_mk_string_unchecked("short_settled_funding", 21, 21);
return x_1;
}
}
static lean_object* _init_l___private_MangoState_PerpMarket_0__MangoV4_reprPerpPosition____x40_MangoState_PerpMarket___hyg_829____closed__13() {
_start:
{
lean_object* x_1; lean_object* x_2; 
x_1 = l___private_MangoState_PerpMarket_0__MangoV4_reprPerpPosition____x40_MangoState_PerpMarket___hyg_829____closed__12;
x_2 = lean_alloc_ctor(3, 1, 0);
lean_ctor_set(x_2, 0, x_1);
return x_2;
}
}
static lean_object* _init_l___private_MangoState_PerpMarket_0__MangoV4_reprPerpPosition____x40_MangoState_PerpMarket___hyg_829____closed__14() {
_start:
{
lean_object* x_1; 
x_1 = lean_mk_string_unchecked("bids_base_lots", 14, 14);
return x_1;
}
}
static lean_object* _init_l___private_MangoState_PerpMarket_0__MangoV4_reprPerpPosition____x40_MangoState_PerpMarket___hyg_829____closed__15() {
_start:
{
lean_object* x_1; lean_object* x_2; 
x_1 = l___private_MangoState_PerpMarket_0__MangoV4_reprPerpPosition____x40_MangoState_PerpMarket___hyg_829____closed__14;
x_2 = lean_alloc_ctor(3, 1, 0);
lean_ctor_set(x_2, 0, x_1);
return x_2;
}
}
static lean_object* _init_l___private_MangoState_PerpMarket_0__MangoV4_reprPerpPosition____x40_MangoState_PerpMarket___hyg_829____closed__16() {
_start:
{
lean_object* x_1; 
x_1 = lean_mk_string_unchecked("asks_base_lots", 14, 14);
return x_1;
}
}
static lean_object* _init_l___private_MangoState_PerpMarket_0__MangoV4_reprPerpPosition____x40_MangoState_PerpMarket___hyg_829____closed__17() {
_start:
{
lean_object* x_1; lean_object* x_2; 
x_1 = l___private_MangoState_PerpMarket_0__MangoV4_reprPerpPosition____x40_MangoState_PerpMarket___hyg_829____closed__16;
x_2 = lean_alloc_ctor(3, 1, 0);
lean_ctor_set(x_2, 0, x_1);
return x_2;
}
}
static lean_object* _init_l___private_MangoState_PerpMarket_0__MangoV4_reprPerpPosition____x40_MangoState_PerpMarket___hyg_829____closed__18() {
_start:
{
lean_object* x_1; 
x_1 = lean_mk_string_unchecked("taker_base_lots", 15, 15);
return x_1;
}
}
static lean_object* _init_l___private_MangoState_PerpMarket_0__MangoV4_reprPerpPosition____x40_MangoState_PerpMarket___hyg_829____closed__19() {
_start:
{
lean_object* x_1; lean_object* x_2; 
x_1 = l___private_MangoState_PerpMarket_0__MangoV4_reprPerpPosition____x40_MangoState_PerpMarket___hyg_829____closed__18;
x_2 = lean_alloc_ctor(3, 1, 0);
lean_ctor_set(x_2, 0, x_1);
return x_2;
}
}
static lean_object* _init_l___private_MangoState_PerpMarket_0__MangoV4_reprPerpPosition____x40_MangoState_PerpMarket___hyg_829____closed__20() {
_start:
{
lean_object* x_1; lean_object* x_2; 
x_1 = lean_unsigned_to_nat(19u);
x_2 = lean_nat_to_int(x_1);
return x_2;
}
}
static lean_object* _init_l___private_MangoState_PerpMarket_0__MangoV4_reprPerpPosition____x40_MangoState_PerpMarket___hyg_829____closed__21() {
_start:
{
lean_object* x_1; 
x_1 = lean_mk_string_unchecked("taker_quote_lots", 16, 16);
return x_1;
}
}
static lean_object* _init_l___private_MangoState_PerpMarket_0__MangoV4_reprPerpPosition____x40_MangoState_PerpMarket___hyg_829____closed__22() {
_start:
{
lean_object* x_1; lean_object* x_2; 
x_1 = l___private_MangoState_PerpMarket_0__MangoV4_reprPerpPosition____x40_MangoState_PerpMarket___hyg_829____closed__21;
x_2 = lean_alloc_ctor(3, 1, 0);
lean_ctor_set(x_2, 0, x_1);
return x_2;
}
}
static lean_object* _init_l___private_MangoState_PerpMarket_0__MangoV4_reprPerpPosition____x40_MangoState_PerpMarket___hyg_829____closed__23() {
_start:
{
lean_object* x_1; lean_object* x_2; 
x_1 = lean_unsigned_to_nat(20u);
x_2 = lean_nat_to_int(x_1);
return x_2;
}
}
static lean_object* _init_l___private_MangoState_PerpMarket_0__MangoV4_reprPerpPosition____x40_MangoState_PerpMarket___hyg_829____closed__24() {
_start:
{
lean_object* x_1; 
x_1 = lean_mk_string_unchecked("settle_pnl_limit_settled_in_current_window_native", 49, 49);
return x_1;
}
}
static lean_object* _init_l___private_MangoState_PerpMarket_0__MangoV4_reprPerpPosition____x40_MangoState_PerpMarket___hyg_829____closed__25() {
_start:
{
lean_object* x_1; lean_object* x_2; 
x_1 = l___private_MangoState_PerpMarket_0__MangoV4_reprPerpPosition____x40_MangoState_PerpMarket___hyg_829____closed__24;
x_2 = lean_alloc_ctor(3, 1, 0);
lean_ctor_set(x_2, 0, x_1);
return x_2;
}
}
static lean_object* _init_l___private_MangoState_PerpMarket_0__MangoV4_reprPerpPosition____x40_MangoState_PerpMarket___hyg_829____closed__26() {
_start:
{
lean_object* x_1; lean_object* x_2; 
x_1 = lean_unsigned_to_nat(53u);
x_2 = lean_nat_to_int(x_1);
return x_2;
}
}
static lean_object* _init_l___private_MangoState_PerpMarket_0__MangoV4_reprPerpPosition____x40_MangoState_PerpMarket___hyg_829____closed__27() {
_start:
{
lean_object* x_1; 
x_1 = lean_mk_string_unchecked("oneshot_settle_pnl_allowance", 28, 28);
return x_1;
}
}
static lean_object* _init_l___private_MangoState_PerpMarket_0__MangoV4_reprPerpPosition____x40_MangoState_PerpMarket___hyg_829____closed__28() {
_start:
{
lean_object* x_1; lean_object* x_2; 
x_1 = l___private_MangoState_PerpMarket_0__MangoV4_reprPerpPosition____x40_MangoState_PerpMarket___hyg_829____closed__27;
x_2 = lean_alloc_ctor(3, 1, 0);
lean_ctor_set(x_2, 0, x_1);
return x_2;
}
}
static lean_object* _init_l___private_MangoState_PerpMarket_0__MangoV4_reprPerpPosition____x40_MangoState_PerpMarket___hyg_829____closed__29() {
_start:
{
lean_object* x_1; lean_object* x_2; 
x_1 = lean_unsigned_to_nat(32u);
x_2 = lean_nat_to_int(x_1);
return x_2;
}
}
static lean_object* _init_l___private_MangoState_PerpMarket_0__MangoV4_reprPerpPosition____x40_MangoState_PerpMarket___hyg_829____closed__30() {
_start:
{
lean_object* x_1; 
x_1 = lean_mk_string_unchecked("recurring_settle_pnl_allowance", 30, 30);
return x_1;
}
}
static lean_object* _init_l___private_MangoState_PerpMarket_0__MangoV4_reprPerpPosition____x40_MangoState_PerpMarket___hyg_829____closed__31() {
_start:
{
lean_object* x_1; lean_object* x_2; 
x_1 = l___private_MangoState_PerpMarket_0__MangoV4_reprPerpPosition____x40_MangoState_PerpMarket___hyg_829____closed__30;
x_2 = lean_alloc_ctor(3, 1, 0);
lean_ctor_set(x_2, 0, x_1);
return x_2;
}
}
static lean_object* _init_l___private_MangoState_PerpMarket_0__MangoV4_reprPerpPosition____x40_MangoState_PerpMarket___hyg_829____closed__32() {
_start:
{
lean_object* x_1; lean_object* x_2; 
x_1 = lean_unsigned_to_nat(34u);
x_2 = lean_nat_to_int(x_1);
return x_2;
}
}
LEAN_EXPORT lean_object* l___private_MangoState_PerpMarket_0__MangoV4_reprPerpPosition____x40_MangoState_PerpMarket___hyg_829_(lean_object* x_1, lean_object* x_2) {
_start:
{
lean_object* x_3; lean_object* x_4; lean_object* x_5; lean_object* x_6; lean_object* x_7; uint8_t x_8; lean_object* x_9; lean_object* x_10; lean_object* x_11; lean_object* x_12; lean_object* x_13; lean_object* x_14; lean_object* x_15; lean_object* x_16; lean_object* x_17; lean_object* x_18; lean_object* x_19; lean_object* x_20; lean_object* x_21; uint8_t x_22; lean_object* x_23; uint8_t x_24; lean_object* x_25; uint8_t x_26; lean_object* x_27; uint8_t x_28; lean_object* x_29; lean_object* x_30; lean_object* x_31; lean_object* x_32; lean_object* x_33; lean_object* x_34; lean_object* x_35; lean_object* x_36; lean_object* x_37; lean_object* x_38; lean_object* x_39; lean_object* x_40; uint8_t x_41; lean_object* x_42; uint8_t x_43; lean_object* x_44; uint8_t x_45; lean_object* x_46; uint8_t x_47; lean_object* x_48; uint8_t x_49; lean_object* x_50; 
x_3 = lean_ctor_get(x_1, 0);
lean_inc(x_3);
x_4 = l___private_Init_Data_Repr_0__Nat_reprFast(x_3);
x_5 = lean_alloc_ctor(3, 1, 0);
lean_ctor_set(x_5, 0, x_4);
x_6 = l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__27;
x_7 = lean_alloc_ctor(4, 2, 0);
lean_ctor_set(x_7, 0, x_6);
lean_ctor_set(x_7, 1, x_5);
x_8 = 0;
x_9 = lean_alloc_ctor(6, 1, 1);
lean_ctor_set(x_9, 0, x_7);
lean_ctor_set_uint8(x_9, sizeof(void*)*1, x_8);
x_10 = l___private_MangoState_PerpMarket_0__MangoV4_reprPerpPosition____x40_MangoState_PerpMarket___hyg_829____closed__4;
x_11 = lean_alloc_ctor(5, 2, 0);
lean_ctor_set(x_11, 0, x_10);
lean_ctor_set(x_11, 1, x_9);
x_12 = l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__9;
x_13 = lean_alloc_ctor(5, 2, 0);
lean_ctor_set(x_13, 0, x_11);
lean_ctor_set(x_13, 1, x_12);
x_14 = lean_box(1);
x_15 = lean_alloc_ctor(5, 2, 0);
lean_ctor_set(x_15, 0, x_13);
lean_ctor_set(x_15, 1, x_14);
x_16 = l___private_MangoState_PerpMarket_0__MangoV4_reprPerpPosition____x40_MangoState_PerpMarket___hyg_829____closed__6;
x_17 = lean_alloc_ctor(5, 2, 0);
lean_ctor_set(x_17, 0, x_15);
lean_ctor_set(x_17, 1, x_16);
x_18 = l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__5;
x_19 = lean_alloc_ctor(5, 2, 0);
lean_ctor_set(x_19, 0, x_17);
lean_ctor_set(x_19, 1, x_18);
x_20 = lean_ctor_get(x_1, 1);
lean_inc(x_20);
x_21 = l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__28;
x_22 = lean_int_dec_lt(x_20, x_21);
x_23 = lean_ctor_get(x_1, 2);
lean_inc(x_23);
x_24 = lean_int_dec_lt(x_23, x_21);
x_25 = lean_ctor_get(x_1, 3);
lean_inc(x_25);
x_26 = lean_int_dec_lt(x_25, x_21);
x_27 = lean_ctor_get(x_1, 4);
lean_inc(x_27);
x_28 = lean_int_dec_lt(x_27, x_21);
x_29 = lean_ctor_get(x_1, 5);
lean_inc(x_29);
x_30 = l___private_Init_Data_Repr_0__Nat_reprFast(x_29);
x_31 = lean_alloc_ctor(3, 1, 0);
lean_ctor_set(x_31, 0, x_30);
x_32 = l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__21;
x_33 = lean_alloc_ctor(4, 2, 0);
lean_ctor_set(x_33, 0, x_32);
lean_ctor_set(x_33, 1, x_31);
x_34 = lean_alloc_ctor(6, 1, 1);
lean_ctor_set(x_34, 0, x_33);
lean_ctor_set_uint8(x_34, sizeof(void*)*1, x_8);
x_35 = lean_ctor_get(x_1, 6);
lean_inc(x_35);
x_36 = l___private_Init_Data_Repr_0__Nat_reprFast(x_35);
x_37 = lean_alloc_ctor(3, 1, 0);
lean_ctor_set(x_37, 0, x_36);
x_38 = lean_alloc_ctor(4, 2, 0);
lean_ctor_set(x_38, 0, x_32);
lean_ctor_set(x_38, 1, x_37);
x_39 = lean_alloc_ctor(6, 1, 1);
lean_ctor_set(x_39, 0, x_38);
lean_ctor_set_uint8(x_39, sizeof(void*)*1, x_8);
x_40 = lean_ctor_get(x_1, 7);
lean_inc(x_40);
x_41 = lean_int_dec_lt(x_40, x_21);
x_42 = lean_ctor_get(x_1, 8);
lean_inc(x_42);
x_43 = lean_int_dec_lt(x_42, x_21);
x_44 = lean_ctor_get(x_1, 9);
lean_inc(x_44);
x_45 = lean_int_dec_lt(x_44, x_21);
x_46 = lean_ctor_get(x_1, 10);
lean_inc(x_46);
x_47 = lean_int_dec_lt(x_46, x_21);
x_48 = lean_ctor_get(x_1, 11);
lean_inc(x_48);
lean_dec(x_1);
x_49 = lean_int_dec_lt(x_48, x_21);
if (x_22 == 0)
{
lean_object* x_219; lean_object* x_220; 
x_219 = l_Int_repr(x_20);
lean_dec(x_20);
x_220 = lean_alloc_ctor(3, 1, 0);
lean_ctor_set(x_220, 0, x_219);
x_50 = x_220;
goto block_218;
}
else
{
lean_object* x_221; lean_object* x_222; lean_object* x_223; lean_object* x_224; 
x_221 = l_Int_repr(x_20);
lean_dec(x_20);
x_222 = lean_alloc_ctor(3, 1, 0);
lean_ctor_set(x_222, 0, x_221);
x_223 = lean_unsigned_to_nat(0u);
x_224 = l_Repr_addAppParen(x_222, x_223);
x_50 = x_224;
goto block_218;
}
block_218:
{
lean_object* x_51; lean_object* x_52; lean_object* x_53; lean_object* x_54; lean_object* x_55; lean_object* x_56; lean_object* x_57; lean_object* x_58; lean_object* x_59; lean_object* x_60; 
x_51 = l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__12;
x_52 = lean_alloc_ctor(4, 2, 0);
lean_ctor_set(x_52, 0, x_51);
lean_ctor_set(x_52, 1, x_50);
x_53 = lean_alloc_ctor(6, 1, 1);
lean_ctor_set(x_53, 0, x_52);
lean_ctor_set_uint8(x_53, sizeof(void*)*1, x_8);
x_54 = lean_alloc_ctor(5, 2, 0);
lean_ctor_set(x_54, 0, x_19);
lean_ctor_set(x_54, 1, x_53);
x_55 = lean_alloc_ctor(5, 2, 0);
lean_ctor_set(x_55, 0, x_54);
lean_ctor_set(x_55, 1, x_12);
x_56 = lean_alloc_ctor(5, 2, 0);
lean_ctor_set(x_56, 0, x_55);
lean_ctor_set(x_56, 1, x_14);
x_57 = l___private_MangoState_PerpMarket_0__MangoV4_reprPerpPosition____x40_MangoState_PerpMarket___hyg_829____closed__8;
x_58 = lean_alloc_ctor(5, 2, 0);
lean_ctor_set(x_58, 0, x_56);
lean_ctor_set(x_58, 1, x_57);
x_59 = lean_alloc_ctor(5, 2, 0);
lean_ctor_set(x_59, 0, x_58);
lean_ctor_set(x_59, 1, x_18);
if (x_24 == 0)
{
lean_object* x_212; lean_object* x_213; 
x_212 = l_Int_repr(x_23);
lean_dec(x_23);
x_213 = lean_alloc_ctor(3, 1, 0);
lean_ctor_set(x_213, 0, x_212);
x_60 = x_213;
goto block_211;
}
else
{
lean_object* x_214; lean_object* x_215; lean_object* x_216; lean_object* x_217; 
x_214 = l_Int_repr(x_23);
lean_dec(x_23);
x_215 = lean_alloc_ctor(3, 1, 0);
lean_ctor_set(x_215, 0, x_214);
x_216 = lean_unsigned_to_nat(0u);
x_217 = l_Repr_addAppParen(x_215, x_216);
x_60 = x_217;
goto block_211;
}
block_211:
{
lean_object* x_61; lean_object* x_62; lean_object* x_63; lean_object* x_64; lean_object* x_65; lean_object* x_66; lean_object* x_67; lean_object* x_68; lean_object* x_69; lean_object* x_70; 
x_61 = l___private_MangoState_PerpMarket_0__MangoV4_reprPerpPosition____x40_MangoState_PerpMarket___hyg_829____closed__9;
x_62 = lean_alloc_ctor(4, 2, 0);
lean_ctor_set(x_62, 0, x_61);
lean_ctor_set(x_62, 1, x_60);
x_63 = lean_alloc_ctor(6, 1, 1);
lean_ctor_set(x_63, 0, x_62);
lean_ctor_set_uint8(x_63, sizeof(void*)*1, x_8);
x_64 = lean_alloc_ctor(5, 2, 0);
lean_ctor_set(x_64, 0, x_59);
lean_ctor_set(x_64, 1, x_63);
x_65 = lean_alloc_ctor(5, 2, 0);
lean_ctor_set(x_65, 0, x_64);
lean_ctor_set(x_65, 1, x_12);
x_66 = lean_alloc_ctor(5, 2, 0);
lean_ctor_set(x_66, 0, x_65);
lean_ctor_set(x_66, 1, x_14);
x_67 = l___private_MangoState_PerpMarket_0__MangoV4_reprPerpPosition____x40_MangoState_PerpMarket___hyg_829____closed__11;
x_68 = lean_alloc_ctor(5, 2, 0);
lean_ctor_set(x_68, 0, x_66);
lean_ctor_set(x_68, 1, x_67);
x_69 = lean_alloc_ctor(5, 2, 0);
lean_ctor_set(x_69, 0, x_68);
lean_ctor_set(x_69, 1, x_18);
if (x_26 == 0)
{
lean_object* x_205; lean_object* x_206; 
x_205 = l_Int_repr(x_25);
lean_dec(x_25);
x_206 = lean_alloc_ctor(3, 1, 0);
lean_ctor_set(x_206, 0, x_205);
x_70 = x_206;
goto block_204;
}
else
{
lean_object* x_207; lean_object* x_208; lean_object* x_209; lean_object* x_210; 
x_207 = l_Int_repr(x_25);
lean_dec(x_25);
x_208 = lean_alloc_ctor(3, 1, 0);
lean_ctor_set(x_208, 0, x_207);
x_209 = lean_unsigned_to_nat(0u);
x_210 = l_Repr_addAppParen(x_208, x_209);
x_70 = x_210;
goto block_204;
}
block_204:
{
lean_object* x_71; lean_object* x_72; lean_object* x_73; lean_object* x_74; lean_object* x_75; lean_object* x_76; lean_object* x_77; lean_object* x_78; lean_object* x_79; lean_object* x_80; 
x_71 = l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__33;
x_72 = lean_alloc_ctor(4, 2, 0);
lean_ctor_set(x_72, 0, x_71);
lean_ctor_set(x_72, 1, x_70);
x_73 = lean_alloc_ctor(6, 1, 1);
lean_ctor_set(x_73, 0, x_72);
lean_ctor_set_uint8(x_73, sizeof(void*)*1, x_8);
x_74 = lean_alloc_ctor(5, 2, 0);
lean_ctor_set(x_74, 0, x_69);
lean_ctor_set(x_74, 1, x_73);
x_75 = lean_alloc_ctor(5, 2, 0);
lean_ctor_set(x_75, 0, x_74);
lean_ctor_set(x_75, 1, x_12);
x_76 = lean_alloc_ctor(5, 2, 0);
lean_ctor_set(x_76, 0, x_75);
lean_ctor_set(x_76, 1, x_14);
x_77 = l___private_MangoState_PerpMarket_0__MangoV4_reprPerpPosition____x40_MangoState_PerpMarket___hyg_829____closed__13;
x_78 = lean_alloc_ctor(5, 2, 0);
lean_ctor_set(x_78, 0, x_76);
lean_ctor_set(x_78, 1, x_77);
x_79 = lean_alloc_ctor(5, 2, 0);
lean_ctor_set(x_79, 0, x_78);
lean_ctor_set(x_79, 1, x_18);
if (x_28 == 0)
{
lean_object* x_198; lean_object* x_199; 
x_198 = l_Int_repr(x_27);
lean_dec(x_27);
x_199 = lean_alloc_ctor(3, 1, 0);
lean_ctor_set(x_199, 0, x_198);
x_80 = x_199;
goto block_197;
}
else
{
lean_object* x_200; lean_object* x_201; lean_object* x_202; lean_object* x_203; 
x_200 = l_Int_repr(x_27);
lean_dec(x_27);
x_201 = lean_alloc_ctor(3, 1, 0);
lean_ctor_set(x_201, 0, x_200);
x_202 = lean_unsigned_to_nat(0u);
x_203 = l_Repr_addAppParen(x_201, x_202);
x_80 = x_203;
goto block_197;
}
block_197:
{
lean_object* x_81; lean_object* x_82; lean_object* x_83; lean_object* x_84; lean_object* x_85; lean_object* x_86; lean_object* x_87; lean_object* x_88; lean_object* x_89; lean_object* x_90; lean_object* x_91; lean_object* x_92; lean_object* x_93; lean_object* x_94; lean_object* x_95; lean_object* x_96; lean_object* x_97; lean_object* x_98; lean_object* x_99; lean_object* x_100; lean_object* x_101; 
x_81 = lean_alloc_ctor(4, 2, 0);
lean_ctor_set(x_81, 0, x_61);
lean_ctor_set(x_81, 1, x_80);
x_82 = lean_alloc_ctor(6, 1, 1);
lean_ctor_set(x_82, 0, x_81);
lean_ctor_set_uint8(x_82, sizeof(void*)*1, x_8);
x_83 = lean_alloc_ctor(5, 2, 0);
lean_ctor_set(x_83, 0, x_79);
lean_ctor_set(x_83, 1, x_82);
x_84 = lean_alloc_ctor(5, 2, 0);
lean_ctor_set(x_84, 0, x_83);
lean_ctor_set(x_84, 1, x_12);
x_85 = lean_alloc_ctor(5, 2, 0);
lean_ctor_set(x_85, 0, x_84);
lean_ctor_set(x_85, 1, x_14);
x_86 = l___private_MangoState_PerpMarket_0__MangoV4_reprPerpPosition____x40_MangoState_PerpMarket___hyg_829____closed__15;
x_87 = lean_alloc_ctor(5, 2, 0);
lean_ctor_set(x_87, 0, x_85);
lean_ctor_set(x_87, 1, x_86);
x_88 = lean_alloc_ctor(5, 2, 0);
lean_ctor_set(x_88, 0, x_87);
lean_ctor_set(x_88, 1, x_18);
x_89 = lean_alloc_ctor(5, 2, 0);
lean_ctor_set(x_89, 0, x_88);
lean_ctor_set(x_89, 1, x_34);
x_90 = lean_alloc_ctor(5, 2, 0);
lean_ctor_set(x_90, 0, x_89);
lean_ctor_set(x_90, 1, x_12);
x_91 = lean_alloc_ctor(5, 2, 0);
lean_ctor_set(x_91, 0, x_90);
lean_ctor_set(x_91, 1, x_14);
x_92 = l___private_MangoState_PerpMarket_0__MangoV4_reprPerpPosition____x40_MangoState_PerpMarket___hyg_829____closed__17;
x_93 = lean_alloc_ctor(5, 2, 0);
lean_ctor_set(x_93, 0, x_91);
lean_ctor_set(x_93, 1, x_92);
x_94 = lean_alloc_ctor(5, 2, 0);
lean_ctor_set(x_94, 0, x_93);
lean_ctor_set(x_94, 1, x_18);
x_95 = lean_alloc_ctor(5, 2, 0);
lean_ctor_set(x_95, 0, x_94);
lean_ctor_set(x_95, 1, x_39);
x_96 = lean_alloc_ctor(5, 2, 0);
lean_ctor_set(x_96, 0, x_95);
lean_ctor_set(x_96, 1, x_12);
x_97 = lean_alloc_ctor(5, 2, 0);
lean_ctor_set(x_97, 0, x_96);
lean_ctor_set(x_97, 1, x_14);
x_98 = l___private_MangoState_PerpMarket_0__MangoV4_reprPerpPosition____x40_MangoState_PerpMarket___hyg_829____closed__19;
x_99 = lean_alloc_ctor(5, 2, 0);
lean_ctor_set(x_99, 0, x_97);
lean_ctor_set(x_99, 1, x_98);
x_100 = lean_alloc_ctor(5, 2, 0);
lean_ctor_set(x_100, 0, x_99);
lean_ctor_set(x_100, 1, x_18);
if (x_41 == 0)
{
lean_object* x_191; lean_object* x_192; 
x_191 = l_Int_repr(x_40);
lean_dec(x_40);
x_192 = lean_alloc_ctor(3, 1, 0);
lean_ctor_set(x_192, 0, x_191);
x_101 = x_192;
goto block_190;
}
else
{
lean_object* x_193; lean_object* x_194; lean_object* x_195; lean_object* x_196; 
x_193 = l_Int_repr(x_40);
lean_dec(x_40);
x_194 = lean_alloc_ctor(3, 1, 0);
lean_ctor_set(x_194, 0, x_193);
x_195 = lean_unsigned_to_nat(0u);
x_196 = l_Repr_addAppParen(x_194, x_195);
x_101 = x_196;
goto block_190;
}
block_190:
{
lean_object* x_102; lean_object* x_103; lean_object* x_104; lean_object* x_105; lean_object* x_106; lean_object* x_107; lean_object* x_108; lean_object* x_109; lean_object* x_110; lean_object* x_111; 
x_102 = l___private_MangoState_PerpMarket_0__MangoV4_reprPerpPosition____x40_MangoState_PerpMarket___hyg_829____closed__20;
x_103 = lean_alloc_ctor(4, 2, 0);
lean_ctor_set(x_103, 0, x_102);
lean_ctor_set(x_103, 1, x_101);
x_104 = lean_alloc_ctor(6, 1, 1);
lean_ctor_set(x_104, 0, x_103);
lean_ctor_set_uint8(x_104, sizeof(void*)*1, x_8);
x_105 = lean_alloc_ctor(5, 2, 0);
lean_ctor_set(x_105, 0, x_100);
lean_ctor_set(x_105, 1, x_104);
x_106 = lean_alloc_ctor(5, 2, 0);
lean_ctor_set(x_106, 0, x_105);
lean_ctor_set(x_106, 1, x_12);
x_107 = lean_alloc_ctor(5, 2, 0);
lean_ctor_set(x_107, 0, x_106);
lean_ctor_set(x_107, 1, x_14);
x_108 = l___private_MangoState_PerpMarket_0__MangoV4_reprPerpPosition____x40_MangoState_PerpMarket___hyg_829____closed__22;
x_109 = lean_alloc_ctor(5, 2, 0);
lean_ctor_set(x_109, 0, x_107);
lean_ctor_set(x_109, 1, x_108);
x_110 = lean_alloc_ctor(5, 2, 0);
lean_ctor_set(x_110, 0, x_109);
lean_ctor_set(x_110, 1, x_18);
if (x_43 == 0)
{
lean_object* x_184; lean_object* x_185; 
x_184 = l_Int_repr(x_42);
lean_dec(x_42);
x_185 = lean_alloc_ctor(3, 1, 0);
lean_ctor_set(x_185, 0, x_184);
x_111 = x_185;
goto block_183;
}
else
{
lean_object* x_186; lean_object* x_187; lean_object* x_188; lean_object* x_189; 
x_186 = l_Int_repr(x_42);
lean_dec(x_42);
x_187 = lean_alloc_ctor(3, 1, 0);
lean_ctor_set(x_187, 0, x_186);
x_188 = lean_unsigned_to_nat(0u);
x_189 = l_Repr_addAppParen(x_187, x_188);
x_111 = x_189;
goto block_183;
}
block_183:
{
lean_object* x_112; lean_object* x_113; lean_object* x_114; lean_object* x_115; lean_object* x_116; lean_object* x_117; lean_object* x_118; lean_object* x_119; lean_object* x_120; lean_object* x_121; 
x_112 = l___private_MangoState_PerpMarket_0__MangoV4_reprPerpPosition____x40_MangoState_PerpMarket___hyg_829____closed__23;
x_113 = lean_alloc_ctor(4, 2, 0);
lean_ctor_set(x_113, 0, x_112);
lean_ctor_set(x_113, 1, x_111);
x_114 = lean_alloc_ctor(6, 1, 1);
lean_ctor_set(x_114, 0, x_113);
lean_ctor_set_uint8(x_114, sizeof(void*)*1, x_8);
x_115 = lean_alloc_ctor(5, 2, 0);
lean_ctor_set(x_115, 0, x_110);
lean_ctor_set(x_115, 1, x_114);
x_116 = lean_alloc_ctor(5, 2, 0);
lean_ctor_set(x_116, 0, x_115);
lean_ctor_set(x_116, 1, x_12);
x_117 = lean_alloc_ctor(5, 2, 0);
lean_ctor_set(x_117, 0, x_116);
lean_ctor_set(x_117, 1, x_14);
x_118 = l___private_MangoState_PerpMarket_0__MangoV4_reprPerpPosition____x40_MangoState_PerpMarket___hyg_829____closed__25;
x_119 = lean_alloc_ctor(5, 2, 0);
lean_ctor_set(x_119, 0, x_117);
lean_ctor_set(x_119, 1, x_118);
x_120 = lean_alloc_ctor(5, 2, 0);
lean_ctor_set(x_120, 0, x_119);
lean_ctor_set(x_120, 1, x_18);
if (x_45 == 0)
{
lean_object* x_177; lean_object* x_178; 
x_177 = l_Int_repr(x_44);
lean_dec(x_44);
x_178 = lean_alloc_ctor(3, 1, 0);
lean_ctor_set(x_178, 0, x_177);
x_121 = x_178;
goto block_176;
}
else
{
lean_object* x_179; lean_object* x_180; lean_object* x_181; lean_object* x_182; 
x_179 = l_Int_repr(x_44);
lean_dec(x_44);
x_180 = lean_alloc_ctor(3, 1, 0);
lean_ctor_set(x_180, 0, x_179);
x_181 = lean_unsigned_to_nat(0u);
x_182 = l_Repr_addAppParen(x_180, x_181);
x_121 = x_182;
goto block_176;
}
block_176:
{
lean_object* x_122; lean_object* x_123; lean_object* x_124; lean_object* x_125; lean_object* x_126; lean_object* x_127; lean_object* x_128; lean_object* x_129; lean_object* x_130; lean_object* x_131; 
x_122 = l___private_MangoState_PerpMarket_0__MangoV4_reprPerpPosition____x40_MangoState_PerpMarket___hyg_829____closed__26;
x_123 = lean_alloc_ctor(4, 2, 0);
lean_ctor_set(x_123, 0, x_122);
lean_ctor_set(x_123, 1, x_121);
x_124 = lean_alloc_ctor(6, 1, 1);
lean_ctor_set(x_124, 0, x_123);
lean_ctor_set_uint8(x_124, sizeof(void*)*1, x_8);
x_125 = lean_alloc_ctor(5, 2, 0);
lean_ctor_set(x_125, 0, x_120);
lean_ctor_set(x_125, 1, x_124);
x_126 = lean_alloc_ctor(5, 2, 0);
lean_ctor_set(x_126, 0, x_125);
lean_ctor_set(x_126, 1, x_12);
x_127 = lean_alloc_ctor(5, 2, 0);
lean_ctor_set(x_127, 0, x_126);
lean_ctor_set(x_127, 1, x_14);
x_128 = l___private_MangoState_PerpMarket_0__MangoV4_reprPerpPosition____x40_MangoState_PerpMarket___hyg_829____closed__28;
x_129 = lean_alloc_ctor(5, 2, 0);
lean_ctor_set(x_129, 0, x_127);
lean_ctor_set(x_129, 1, x_128);
x_130 = lean_alloc_ctor(5, 2, 0);
lean_ctor_set(x_130, 0, x_129);
lean_ctor_set(x_130, 1, x_18);
if (x_47 == 0)
{
lean_object* x_170; lean_object* x_171; 
x_170 = l_Int_repr(x_46);
lean_dec(x_46);
x_171 = lean_alloc_ctor(3, 1, 0);
lean_ctor_set(x_171, 0, x_170);
x_131 = x_171;
goto block_169;
}
else
{
lean_object* x_172; lean_object* x_173; lean_object* x_174; lean_object* x_175; 
x_172 = l_Int_repr(x_46);
lean_dec(x_46);
x_173 = lean_alloc_ctor(3, 1, 0);
lean_ctor_set(x_173, 0, x_172);
x_174 = lean_unsigned_to_nat(0u);
x_175 = l_Repr_addAppParen(x_173, x_174);
x_131 = x_175;
goto block_169;
}
block_169:
{
lean_object* x_132; lean_object* x_133; lean_object* x_134; lean_object* x_135; lean_object* x_136; lean_object* x_137; lean_object* x_138; lean_object* x_139; lean_object* x_140; 
x_132 = l___private_MangoState_PerpMarket_0__MangoV4_reprPerpPosition____x40_MangoState_PerpMarket___hyg_829____closed__29;
x_133 = lean_alloc_ctor(4, 2, 0);
lean_ctor_set(x_133, 0, x_132);
lean_ctor_set(x_133, 1, x_131);
x_134 = lean_alloc_ctor(6, 1, 1);
lean_ctor_set(x_134, 0, x_133);
lean_ctor_set_uint8(x_134, sizeof(void*)*1, x_8);
x_135 = lean_alloc_ctor(5, 2, 0);
lean_ctor_set(x_135, 0, x_130);
lean_ctor_set(x_135, 1, x_134);
x_136 = lean_alloc_ctor(5, 2, 0);
lean_ctor_set(x_136, 0, x_135);
lean_ctor_set(x_136, 1, x_12);
x_137 = lean_alloc_ctor(5, 2, 0);
lean_ctor_set(x_137, 0, x_136);
lean_ctor_set(x_137, 1, x_14);
x_138 = l___private_MangoState_PerpMarket_0__MangoV4_reprPerpPosition____x40_MangoState_PerpMarket___hyg_829____closed__31;
x_139 = lean_alloc_ctor(5, 2, 0);
lean_ctor_set(x_139, 0, x_137);
lean_ctor_set(x_139, 1, x_138);
x_140 = lean_alloc_ctor(5, 2, 0);
lean_ctor_set(x_140, 0, x_139);
lean_ctor_set(x_140, 1, x_18);
if (x_49 == 0)
{
lean_object* x_141; lean_object* x_142; lean_object* x_143; lean_object* x_144; lean_object* x_145; lean_object* x_146; lean_object* x_147; lean_object* x_148; lean_object* x_149; lean_object* x_150; lean_object* x_151; lean_object* x_152; lean_object* x_153; 
x_141 = l_Int_repr(x_48);
lean_dec(x_48);
x_142 = lean_alloc_ctor(3, 1, 0);
lean_ctor_set(x_142, 0, x_141);
x_143 = l___private_MangoState_PerpMarket_0__MangoV4_reprPerpPosition____x40_MangoState_PerpMarket___hyg_829____closed__32;
x_144 = lean_alloc_ctor(4, 2, 0);
lean_ctor_set(x_144, 0, x_143);
lean_ctor_set(x_144, 1, x_142);
x_145 = lean_alloc_ctor(6, 1, 1);
lean_ctor_set(x_145, 0, x_144);
lean_ctor_set_uint8(x_145, sizeof(void*)*1, x_8);
x_146 = lean_alloc_ctor(5, 2, 0);
lean_ctor_set(x_146, 0, x_140);
lean_ctor_set(x_146, 1, x_145);
x_147 = l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__74;
x_148 = lean_alloc_ctor(5, 2, 0);
lean_ctor_set(x_148, 0, x_147);
lean_ctor_set(x_148, 1, x_146);
x_149 = l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__76;
x_150 = lean_alloc_ctor(5, 2, 0);
lean_ctor_set(x_150, 0, x_148);
lean_ctor_set(x_150, 1, x_149);
x_151 = l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__73;
x_152 = lean_alloc_ctor(4, 2, 0);
lean_ctor_set(x_152, 0, x_151);
lean_ctor_set(x_152, 1, x_150);
x_153 = lean_alloc_ctor(6, 1, 1);
lean_ctor_set(x_153, 0, x_152);
lean_ctor_set_uint8(x_153, sizeof(void*)*1, x_8);
return x_153;
}
else
{
lean_object* x_154; lean_object* x_155; lean_object* x_156; lean_object* x_157; lean_object* x_158; lean_object* x_159; lean_object* x_160; lean_object* x_161; lean_object* x_162; lean_object* x_163; lean_object* x_164; lean_object* x_165; lean_object* x_166; lean_object* x_167; lean_object* x_168; 
x_154 = l_Int_repr(x_48);
lean_dec(x_48);
x_155 = lean_alloc_ctor(3, 1, 0);
lean_ctor_set(x_155, 0, x_154);
x_156 = lean_unsigned_to_nat(0u);
x_157 = l_Repr_addAppParen(x_155, x_156);
x_158 = l___private_MangoState_PerpMarket_0__MangoV4_reprPerpPosition____x40_MangoState_PerpMarket___hyg_829____closed__32;
x_159 = lean_alloc_ctor(4, 2, 0);
lean_ctor_set(x_159, 0, x_158);
lean_ctor_set(x_159, 1, x_157);
x_160 = lean_alloc_ctor(6, 1, 1);
lean_ctor_set(x_160, 0, x_159);
lean_ctor_set_uint8(x_160, sizeof(void*)*1, x_8);
x_161 = lean_alloc_ctor(5, 2, 0);
lean_ctor_set(x_161, 0, x_140);
lean_ctor_set(x_161, 1, x_160);
x_162 = l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__74;
x_163 = lean_alloc_ctor(5, 2, 0);
lean_ctor_set(x_163, 0, x_162);
lean_ctor_set(x_163, 1, x_161);
x_164 = l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__76;
x_165 = lean_alloc_ctor(5, 2, 0);
lean_ctor_set(x_165, 0, x_163);
lean_ctor_set(x_165, 1, x_164);
x_166 = l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__73;
x_167 = lean_alloc_ctor(4, 2, 0);
lean_ctor_set(x_167, 0, x_166);
lean_ctor_set(x_167, 1, x_165);
x_168 = lean_alloc_ctor(6, 1, 1);
lean_ctor_set(x_168, 0, x_167);
lean_ctor_set_uint8(x_168, sizeof(void*)*1, x_8);
return x_168;
}
}
}
}
}
}
}
}
}
}
}
LEAN_EXPORT lean_object* l___private_MangoState_PerpMarket_0__MangoV4_reprPerpPosition____x40_MangoState_PerpMarket___hyg_829____boxed(lean_object* x_1, lean_object* x_2) {
_start:
{
lean_object* x_3; 
x_3 = l___private_MangoState_PerpMarket_0__MangoV4_reprPerpPosition____x40_MangoState_PerpMarket___hyg_829_(x_1, x_2);
lean_dec(x_2);
return x_3;
}
}
static lean_object* _init_l_MangoV4_instReprPerpPosition___closed__1() {
_start:
{
lean_object* x_1; 
x_1 = lean_alloc_closure((void*)(l___private_MangoState_PerpMarket_0__MangoV4_reprPerpPosition____x40_MangoState_PerpMarket___hyg_829____boxed), 2, 0);
return x_1;
}
}
static lean_object* _init_l_MangoV4_instReprPerpPosition() {
_start:
{
lean_object* x_1; 
x_1 = l_MangoV4_instReprPerpPosition___closed__1;
return x_1;
}
}
LEAN_EXPORT lean_object* l_MangoV4_fundingUpdateTransition(lean_object* x_1, lean_object* x_2, lean_object* x_3, lean_object* x_4) {
_start:
{
lean_object* x_5; uint8_t x_6; 
x_5 = lean_unsigned_to_nat(0u);
x_6 = lean_nat_dec_lt(x_5, x_4);
if (x_6 == 0)
{
lean_object* x_7; 
lean_dec(x_4);
lean_dec(x_3);
lean_dec(x_1);
x_7 = lean_box(0);
return x_7;
}
else
{
lean_object* x_8; uint8_t x_9; 
x_8 = lean_unsigned_to_nat(3600u);
x_9 = lean_nat_dec_le(x_4, x_8);
if (x_9 == 0)
{
lean_object* x_10; 
lean_dec(x_4);
lean_dec(x_3);
lean_dec(x_1);
x_10 = lean_box(0);
return x_10;
}
else
{
uint8_t x_11; 
x_11 = !lean_is_exclusive(x_1);
if (x_11 == 0)
{
lean_object* x_12; lean_object* x_13; lean_object* x_14; lean_object* x_15; lean_object* x_16; lean_object* x_17; lean_object* x_18; lean_object* x_19; lean_object* x_20; lean_object* x_21; lean_object* x_22; lean_object* x_23; lean_object* x_24; lean_object* x_25; lean_object* x_26; lean_object* x_27; lean_object* x_28; lean_object* x_29; lean_object* x_30; lean_object* x_31; lean_object* x_32; lean_object* x_33; uint8_t x_34; 
x_12 = lean_ctor_get(x_1, 0);
x_13 = lean_ctor_get(x_1, 1);
x_14 = lean_ctor_get(x_1, 2);
x_15 = lean_ctor_get(x_1, 3);
x_16 = lean_ctor_get(x_1, 4);
x_17 = lean_ctor_get(x_1, 5);
x_18 = lean_ctor_get(x_1, 6);
x_19 = lean_ctor_get(x_1, 7);
x_20 = lean_ctor_get(x_1, 8);
x_21 = lean_ctor_get(x_1, 9);
x_22 = lean_ctor_get(x_1, 10);
x_23 = lean_ctor_get(x_1, 11);
x_24 = lean_ctor_get(x_1, 12);
x_25 = lean_ctor_get(x_1, 13);
x_26 = lean_ctor_get(x_1, 14);
x_27 = lean_ctor_get(x_1, 15);
x_28 = lean_ctor_get(x_1, 16);
x_29 = lean_ctor_get(x_1, 17);
x_30 = lean_ctor_get(x_1, 18);
x_31 = lean_ctor_get(x_1, 19);
x_32 = lean_ctor_get(x_1, 20);
x_33 = lean_ctor_get(x_1, 21);
x_34 = lean_int_dec_le(x_21, x_2);
if (x_34 == 0)
{
lean_object* x_35; 
lean_free_object(x_1);
lean_dec(x_33);
lean_dec(x_32);
lean_dec(x_31);
lean_dec(x_30);
lean_dec(x_29);
lean_dec(x_28);
lean_dec(x_27);
lean_dec(x_26);
lean_dec(x_25);
lean_dec(x_24);
lean_dec(x_23);
lean_dec(x_22);
lean_dec(x_21);
lean_dec(x_20);
lean_dec(x_19);
lean_dec(x_18);
lean_dec(x_17);
lean_dec(x_16);
lean_dec(x_15);
lean_dec(x_14);
lean_dec(x_13);
lean_dec(x_12);
lean_dec(x_4);
lean_dec(x_3);
x_35 = lean_box(0);
return x_35;
}
else
{
uint8_t x_36; 
x_36 = lean_int_dec_le(x_2, x_22);
if (x_36 == 0)
{
lean_object* x_37; 
lean_free_object(x_1);
lean_dec(x_33);
lean_dec(x_32);
lean_dec(x_31);
lean_dec(x_30);
lean_dec(x_29);
lean_dec(x_28);
lean_dec(x_27);
lean_dec(x_26);
lean_dec(x_25);
lean_dec(x_24);
lean_dec(x_23);
lean_dec(x_22);
lean_dec(x_21);
lean_dec(x_20);
lean_dec(x_19);
lean_dec(x_18);
lean_dec(x_17);
lean_dec(x_16);
lean_dec(x_15);
lean_dec(x_14);
lean_dec(x_13);
lean_dec(x_12);
lean_dec(x_4);
lean_dec(x_3);
x_37 = lean_box(0);
return x_37;
}
else
{
lean_object* x_38; lean_object* x_39; lean_object* x_40; lean_object* x_41; lean_object* x_42; lean_object* x_43; lean_object* x_44; lean_object* x_45; lean_object* x_46; lean_object* x_47; 
x_38 = lean_nat_to_int(x_3);
lean_inc(x_17);
x_39 = lean_nat_to_int(x_17);
x_40 = lean_int_mul(x_38, x_39);
lean_dec(x_39);
lean_dec(x_38);
x_41 = lean_int_mul(x_40, x_2);
lean_dec(x_40);
lean_inc(x_4);
x_42 = lean_nat_to_int(x_4);
x_43 = lean_int_mul(x_41, x_42);
lean_dec(x_42);
lean_dec(x_41);
x_44 = lean_int_add(x_18, x_43);
lean_dec(x_18);
x_45 = lean_int_add(x_19, x_43);
lean_dec(x_43);
lean_dec(x_19);
x_46 = lean_nat_add(x_20, x_4);
lean_dec(x_4);
lean_dec(x_20);
lean_ctor_set(x_1, 8, x_46);
lean_ctor_set(x_1, 7, x_45);
lean_ctor_set(x_1, 6, x_44);
x_47 = lean_alloc_ctor(1, 1, 0);
lean_ctor_set(x_47, 0, x_1);
return x_47;
}
}
}
else
{
lean_object* x_48; lean_object* x_49; lean_object* x_50; lean_object* x_51; lean_object* x_52; lean_object* x_53; lean_object* x_54; lean_object* x_55; lean_object* x_56; lean_object* x_57; lean_object* x_58; lean_object* x_59; lean_object* x_60; lean_object* x_61; lean_object* x_62; lean_object* x_63; lean_object* x_64; lean_object* x_65; lean_object* x_66; lean_object* x_67; lean_object* x_68; lean_object* x_69; uint8_t x_70; uint8_t x_71; uint8_t x_72; uint8_t x_73; 
x_48 = lean_ctor_get(x_1, 0);
x_49 = lean_ctor_get(x_1, 1);
x_50 = lean_ctor_get(x_1, 2);
x_51 = lean_ctor_get(x_1, 3);
x_52 = lean_ctor_get(x_1, 4);
x_53 = lean_ctor_get(x_1, 5);
x_54 = lean_ctor_get(x_1, 6);
x_55 = lean_ctor_get(x_1, 7);
x_56 = lean_ctor_get(x_1, 8);
x_57 = lean_ctor_get(x_1, 9);
x_58 = lean_ctor_get(x_1, 10);
x_59 = lean_ctor_get(x_1, 11);
x_60 = lean_ctor_get(x_1, 12);
x_61 = lean_ctor_get(x_1, 13);
x_62 = lean_ctor_get(x_1, 14);
x_63 = lean_ctor_get(x_1, 15);
x_64 = lean_ctor_get(x_1, 16);
x_65 = lean_ctor_get(x_1, 17);
x_66 = lean_ctor_get(x_1, 18);
x_67 = lean_ctor_get(x_1, 19);
x_68 = lean_ctor_get(x_1, 20);
x_69 = lean_ctor_get(x_1, 21);
x_70 = lean_ctor_get_uint8(x_1, sizeof(void*)*22);
x_71 = lean_ctor_get_uint8(x_1, sizeof(void*)*22 + 1);
x_72 = lean_ctor_get_uint8(x_1, sizeof(void*)*22 + 2);
lean_inc(x_69);
lean_inc(x_68);
lean_inc(x_67);
lean_inc(x_66);
lean_inc(x_65);
lean_inc(x_64);
lean_inc(x_63);
lean_inc(x_62);
lean_inc(x_61);
lean_inc(x_60);
lean_inc(x_59);
lean_inc(x_58);
lean_inc(x_57);
lean_inc(x_56);
lean_inc(x_55);
lean_inc(x_54);
lean_inc(x_53);
lean_inc(x_52);
lean_inc(x_51);
lean_inc(x_50);
lean_inc(x_49);
lean_inc(x_48);
lean_dec(x_1);
x_73 = lean_int_dec_le(x_57, x_2);
if (x_73 == 0)
{
lean_object* x_74; 
lean_dec(x_69);
lean_dec(x_68);
lean_dec(x_67);
lean_dec(x_66);
lean_dec(x_65);
lean_dec(x_64);
lean_dec(x_63);
lean_dec(x_62);
lean_dec(x_61);
lean_dec(x_60);
lean_dec(x_59);
lean_dec(x_58);
lean_dec(x_57);
lean_dec(x_56);
lean_dec(x_55);
lean_dec(x_54);
lean_dec(x_53);
lean_dec(x_52);
lean_dec(x_51);
lean_dec(x_50);
lean_dec(x_49);
lean_dec(x_48);
lean_dec(x_4);
lean_dec(x_3);
x_74 = lean_box(0);
return x_74;
}
else
{
uint8_t x_75; 
x_75 = lean_int_dec_le(x_2, x_58);
if (x_75 == 0)
{
lean_object* x_76; 
lean_dec(x_69);
lean_dec(x_68);
lean_dec(x_67);
lean_dec(x_66);
lean_dec(x_65);
lean_dec(x_64);
lean_dec(x_63);
lean_dec(x_62);
lean_dec(x_61);
lean_dec(x_60);
lean_dec(x_59);
lean_dec(x_58);
lean_dec(x_57);
lean_dec(x_56);
lean_dec(x_55);
lean_dec(x_54);
lean_dec(x_53);
lean_dec(x_52);
lean_dec(x_51);
lean_dec(x_50);
lean_dec(x_49);
lean_dec(x_48);
lean_dec(x_4);
lean_dec(x_3);
x_76 = lean_box(0);
return x_76;
}
else
{
lean_object* x_77; lean_object* x_78; lean_object* x_79; lean_object* x_80; lean_object* x_81; lean_object* x_82; lean_object* x_83; lean_object* x_84; lean_object* x_85; lean_object* x_86; lean_object* x_87; 
x_77 = lean_nat_to_int(x_3);
lean_inc(x_53);
x_78 = lean_nat_to_int(x_53);
x_79 = lean_int_mul(x_77, x_78);
lean_dec(x_78);
lean_dec(x_77);
x_80 = lean_int_mul(x_79, x_2);
lean_dec(x_79);
lean_inc(x_4);
x_81 = lean_nat_to_int(x_4);
x_82 = lean_int_mul(x_80, x_81);
lean_dec(x_81);
lean_dec(x_80);
x_83 = lean_int_add(x_54, x_82);
lean_dec(x_54);
x_84 = lean_int_add(x_55, x_82);
lean_dec(x_82);
lean_dec(x_55);
x_85 = lean_nat_add(x_56, x_4);
lean_dec(x_4);
lean_dec(x_56);
x_86 = lean_alloc_ctor(0, 22, 3);
lean_ctor_set(x_86, 0, x_48);
lean_ctor_set(x_86, 1, x_49);
lean_ctor_set(x_86, 2, x_50);
lean_ctor_set(x_86, 3, x_51);
lean_ctor_set(x_86, 4, x_52);
lean_ctor_set(x_86, 5, x_53);
lean_ctor_set(x_86, 6, x_83);
lean_ctor_set(x_86, 7, x_84);
lean_ctor_set(x_86, 8, x_85);
lean_ctor_set(x_86, 9, x_57);
lean_ctor_set(x_86, 10, x_58);
lean_ctor_set(x_86, 11, x_59);
lean_ctor_set(x_86, 12, x_60);
lean_ctor_set(x_86, 13, x_61);
lean_ctor_set(x_86, 14, x_62);
lean_ctor_set(x_86, 15, x_63);
lean_ctor_set(x_86, 16, x_64);
lean_ctor_set(x_86, 17, x_65);
lean_ctor_set(x_86, 18, x_66);
lean_ctor_set(x_86, 19, x_67);
lean_ctor_set(x_86, 20, x_68);
lean_ctor_set(x_86, 21, x_69);
lean_ctor_set_uint8(x_86, sizeof(void*)*22, x_70);
lean_ctor_set_uint8(x_86, sizeof(void*)*22 + 1, x_71);
lean_ctor_set_uint8(x_86, sizeof(void*)*22 + 2, x_72);
x_87 = lean_alloc_ctor(1, 1, 0);
lean_ctor_set(x_87, 0, x_86);
return x_87;
}
}
}
}
}
}
}
LEAN_EXPORT lean_object* l_MangoV4_fundingUpdateTransition___boxed(lean_object* x_1, lean_object* x_2, lean_object* x_3, lean_object* x_4) {
_start:
{
lean_object* x_5; 
x_5 = l_MangoV4_fundingUpdateTransition(x_1, x_2, x_3, x_4);
lean_dec(x_2);
return x_5;
}
}
LEAN_EXPORT lean_object* l_MangoV4_placeOrderTransition(lean_object* x_1) {
_start:
{
uint8_t x_2; 
x_2 = !lean_is_exclusive(x_1);
if (x_2 == 0)
{
lean_object* x_3; lean_object* x_4; lean_object* x_5; 
x_3 = lean_ctor_get(x_1, 12);
x_4 = lean_unsigned_to_nat(1u);
x_5 = lean_nat_add(x_3, x_4);
lean_dec(x_3);
lean_ctor_set(x_1, 12, x_5);
return x_1;
}
else
{
lean_object* x_6; lean_object* x_7; lean_object* x_8; lean_object* x_9; lean_object* x_10; lean_object* x_11; lean_object* x_12; lean_object* x_13; lean_object* x_14; lean_object* x_15; lean_object* x_16; lean_object* x_17; lean_object* x_18; lean_object* x_19; lean_object* x_20; lean_object* x_21; lean_object* x_22; lean_object* x_23; lean_object* x_24; lean_object* x_25; lean_object* x_26; lean_object* x_27; uint8_t x_28; uint8_t x_29; uint8_t x_30; lean_object* x_31; lean_object* x_32; lean_object* x_33; 
x_6 = lean_ctor_get(x_1, 0);
x_7 = lean_ctor_get(x_1, 1);
x_8 = lean_ctor_get(x_1, 2);
x_9 = lean_ctor_get(x_1, 3);
x_10 = lean_ctor_get(x_1, 4);
x_11 = lean_ctor_get(x_1, 5);
x_12 = lean_ctor_get(x_1, 6);
x_13 = lean_ctor_get(x_1, 7);
x_14 = lean_ctor_get(x_1, 8);
x_15 = lean_ctor_get(x_1, 9);
x_16 = lean_ctor_get(x_1, 10);
x_17 = lean_ctor_get(x_1, 11);
x_18 = lean_ctor_get(x_1, 12);
x_19 = lean_ctor_get(x_1, 13);
x_20 = lean_ctor_get(x_1, 14);
x_21 = lean_ctor_get(x_1, 15);
x_22 = lean_ctor_get(x_1, 16);
x_23 = lean_ctor_get(x_1, 17);
x_24 = lean_ctor_get(x_1, 18);
x_25 = lean_ctor_get(x_1, 19);
x_26 = lean_ctor_get(x_1, 20);
x_27 = lean_ctor_get(x_1, 21);
x_28 = lean_ctor_get_uint8(x_1, sizeof(void*)*22);
x_29 = lean_ctor_get_uint8(x_1, sizeof(void*)*22 + 1);
x_30 = lean_ctor_get_uint8(x_1, sizeof(void*)*22 + 2);
lean_inc(x_27);
lean_inc(x_26);
lean_inc(x_25);
lean_inc(x_24);
lean_inc(x_23);
lean_inc(x_22);
lean_inc(x_21);
lean_inc(x_20);
lean_inc(x_19);
lean_inc(x_18);
lean_inc(x_17);
lean_inc(x_16);
lean_inc(x_15);
lean_inc(x_14);
lean_inc(x_13);
lean_inc(x_12);
lean_inc(x_11);
lean_inc(x_10);
lean_inc(x_9);
lean_inc(x_8);
lean_inc(x_7);
lean_inc(x_6);
lean_dec(x_1);
x_31 = lean_unsigned_to_nat(1u);
x_32 = lean_nat_add(x_18, x_31);
lean_dec(x_18);
x_33 = lean_alloc_ctor(0, 22, 3);
lean_ctor_set(x_33, 0, x_6);
lean_ctor_set(x_33, 1, x_7);
lean_ctor_set(x_33, 2, x_8);
lean_ctor_set(x_33, 3, x_9);
lean_ctor_set(x_33, 4, x_10);
lean_ctor_set(x_33, 5, x_11);
lean_ctor_set(x_33, 6, x_12);
lean_ctor_set(x_33, 7, x_13);
lean_ctor_set(x_33, 8, x_14);
lean_ctor_set(x_33, 9, x_15);
lean_ctor_set(x_33, 10, x_16);
lean_ctor_set(x_33, 11, x_17);
lean_ctor_set(x_33, 12, x_32);
lean_ctor_set(x_33, 13, x_19);
lean_ctor_set(x_33, 14, x_20);
lean_ctor_set(x_33, 15, x_21);
lean_ctor_set(x_33, 16, x_22);
lean_ctor_set(x_33, 17, x_23);
lean_ctor_set(x_33, 18, x_24);
lean_ctor_set(x_33, 19, x_25);
lean_ctor_set(x_33, 20, x_26);
lean_ctor_set(x_33, 21, x_27);
lean_ctor_set_uint8(x_33, sizeof(void*)*22, x_28);
lean_ctor_set_uint8(x_33, sizeof(void*)*22 + 1, x_29);
lean_ctor_set_uint8(x_33, sizeof(void*)*22 + 2, x_30);
return x_33;
}
}
}
LEAN_EXPORT lean_object* l_MangoV4_perpSocializeLossTransition(lean_object* x_1, lean_object* x_2) {
_start:
{
lean_object* x_3; uint8_t x_4; 
x_3 = l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__28;
x_4 = lean_int_dec_le(x_2, x_3);
if (x_4 == 0)
{
lean_object* x_5; 
lean_dec(x_1);
x_5 = lean_box(0);
return x_5;
}
else
{
uint8_t x_6; 
x_6 = !lean_is_exclusive(x_1);
if (x_6 == 0)
{
lean_object* x_7; lean_object* x_8; lean_object* x_9; lean_object* x_10; lean_object* x_11; uint8_t x_12; 
x_7 = lean_ctor_get(x_1, 6);
x_8 = lean_ctor_get(x_1, 7);
x_9 = lean_ctor_get(x_1, 11);
x_10 = lean_ctor_get(x_1, 21);
x_11 = lean_unsigned_to_nat(0u);
x_12 = lean_nat_dec_lt(x_11, x_9);
if (x_12 == 0)
{
lean_object* x_13; lean_object* x_14; 
x_13 = lean_int_add(x_10, x_2);
lean_dec(x_10);
lean_ctor_set(x_1, 21, x_13);
x_14 = lean_alloc_ctor(1, 1, 0);
lean_ctor_set(x_14, 0, x_1);
return x_14;
}
else
{
lean_object* x_15; lean_object* x_16; lean_object* x_17; lean_object* x_18; lean_object* x_19; 
lean_inc(x_9);
x_15 = lean_nat_to_int(x_9);
x_16 = lean_int_ediv(x_2, x_15);
lean_dec(x_15);
x_17 = lean_int_sub(x_7, x_16);
lean_dec(x_7);
x_18 = lean_int_add(x_8, x_16);
lean_dec(x_16);
lean_dec(x_8);
lean_ctor_set(x_1, 7, x_18);
lean_ctor_set(x_1, 6, x_17);
x_19 = lean_alloc_ctor(1, 1, 0);
lean_ctor_set(x_19, 0, x_1);
return x_19;
}
}
else
{
lean_object* x_20; lean_object* x_21; lean_object* x_22; lean_object* x_23; lean_object* x_24; lean_object* x_25; lean_object* x_26; lean_object* x_27; lean_object* x_28; lean_object* x_29; lean_object* x_30; lean_object* x_31; lean_object* x_32; lean_object* x_33; lean_object* x_34; lean_object* x_35; lean_object* x_36; lean_object* x_37; lean_object* x_38; lean_object* x_39; lean_object* x_40; lean_object* x_41; uint8_t x_42; uint8_t x_43; uint8_t x_44; lean_object* x_45; uint8_t x_46; 
x_20 = lean_ctor_get(x_1, 0);
x_21 = lean_ctor_get(x_1, 1);
x_22 = lean_ctor_get(x_1, 2);
x_23 = lean_ctor_get(x_1, 3);
x_24 = lean_ctor_get(x_1, 4);
x_25 = lean_ctor_get(x_1, 5);
x_26 = lean_ctor_get(x_1, 6);
x_27 = lean_ctor_get(x_1, 7);
x_28 = lean_ctor_get(x_1, 8);
x_29 = lean_ctor_get(x_1, 9);
x_30 = lean_ctor_get(x_1, 10);
x_31 = lean_ctor_get(x_1, 11);
x_32 = lean_ctor_get(x_1, 12);
x_33 = lean_ctor_get(x_1, 13);
x_34 = lean_ctor_get(x_1, 14);
x_35 = lean_ctor_get(x_1, 15);
x_36 = lean_ctor_get(x_1, 16);
x_37 = lean_ctor_get(x_1, 17);
x_38 = lean_ctor_get(x_1, 18);
x_39 = lean_ctor_get(x_1, 19);
x_40 = lean_ctor_get(x_1, 20);
x_41 = lean_ctor_get(x_1, 21);
x_42 = lean_ctor_get_uint8(x_1, sizeof(void*)*22);
x_43 = lean_ctor_get_uint8(x_1, sizeof(void*)*22 + 1);
x_44 = lean_ctor_get_uint8(x_1, sizeof(void*)*22 + 2);
lean_inc(x_41);
lean_inc(x_40);
lean_inc(x_39);
lean_inc(x_38);
lean_inc(x_37);
lean_inc(x_36);
lean_inc(x_35);
lean_inc(x_34);
lean_inc(x_33);
lean_inc(x_32);
lean_inc(x_31);
lean_inc(x_30);
lean_inc(x_29);
lean_inc(x_28);
lean_inc(x_27);
lean_inc(x_26);
lean_inc(x_25);
lean_inc(x_24);
lean_inc(x_23);
lean_inc(x_22);
lean_inc(x_21);
lean_inc(x_20);
lean_dec(x_1);
x_45 = lean_unsigned_to_nat(0u);
x_46 = lean_nat_dec_lt(x_45, x_31);
if (x_46 == 0)
{
lean_object* x_47; lean_object* x_48; lean_object* x_49; 
x_47 = lean_int_add(x_41, x_2);
lean_dec(x_41);
x_48 = lean_alloc_ctor(0, 22, 3);
lean_ctor_set(x_48, 0, x_20);
lean_ctor_set(x_48, 1, x_21);
lean_ctor_set(x_48, 2, x_22);
lean_ctor_set(x_48, 3, x_23);
lean_ctor_set(x_48, 4, x_24);
lean_ctor_set(x_48, 5, x_25);
lean_ctor_set(x_48, 6, x_26);
lean_ctor_set(x_48, 7, x_27);
lean_ctor_set(x_48, 8, x_28);
lean_ctor_set(x_48, 9, x_29);
lean_ctor_set(x_48, 10, x_30);
lean_ctor_set(x_48, 11, x_31);
lean_ctor_set(x_48, 12, x_32);
lean_ctor_set(x_48, 13, x_33);
lean_ctor_set(x_48, 14, x_34);
lean_ctor_set(x_48, 15, x_35);
lean_ctor_set(x_48, 16, x_36);
lean_ctor_set(x_48, 17, x_37);
lean_ctor_set(x_48, 18, x_38);
lean_ctor_set(x_48, 19, x_39);
lean_ctor_set(x_48, 20, x_40);
lean_ctor_set(x_48, 21, x_47);
lean_ctor_set_uint8(x_48, sizeof(void*)*22, x_42);
lean_ctor_set_uint8(x_48, sizeof(void*)*22 + 1, x_43);
lean_ctor_set_uint8(x_48, sizeof(void*)*22 + 2, x_44);
x_49 = lean_alloc_ctor(1, 1, 0);
lean_ctor_set(x_49, 0, x_48);
return x_49;
}
else
{
lean_object* x_50; lean_object* x_51; lean_object* x_52; lean_object* x_53; lean_object* x_54; lean_object* x_55; 
lean_inc(x_31);
x_50 = lean_nat_to_int(x_31);
x_51 = lean_int_ediv(x_2, x_50);
lean_dec(x_50);
x_52 = lean_int_sub(x_26, x_51);
lean_dec(x_26);
x_53 = lean_int_add(x_27, x_51);
lean_dec(x_51);
lean_dec(x_27);
x_54 = lean_alloc_ctor(0, 22, 3);
lean_ctor_set(x_54, 0, x_20);
lean_ctor_set(x_54, 1, x_21);
lean_ctor_set(x_54, 2, x_22);
lean_ctor_set(x_54, 3, x_23);
lean_ctor_set(x_54, 4, x_24);
lean_ctor_set(x_54, 5, x_25);
lean_ctor_set(x_54, 6, x_52);
lean_ctor_set(x_54, 7, x_53);
lean_ctor_set(x_54, 8, x_28);
lean_ctor_set(x_54, 9, x_29);
lean_ctor_set(x_54, 10, x_30);
lean_ctor_set(x_54, 11, x_31);
lean_ctor_set(x_54, 12, x_32);
lean_ctor_set(x_54, 13, x_33);
lean_ctor_set(x_54, 14, x_34);
lean_ctor_set(x_54, 15, x_35);
lean_ctor_set(x_54, 16, x_36);
lean_ctor_set(x_54, 17, x_37);
lean_ctor_set(x_54, 18, x_38);
lean_ctor_set(x_54, 19, x_39);
lean_ctor_set(x_54, 20, x_40);
lean_ctor_set(x_54, 21, x_41);
lean_ctor_set_uint8(x_54, sizeof(void*)*22, x_42);
lean_ctor_set_uint8(x_54, sizeof(void*)*22 + 1, x_43);
lean_ctor_set_uint8(x_54, sizeof(void*)*22 + 2, x_44);
x_55 = lean_alloc_ctor(1, 1, 0);
lean_ctor_set(x_55, 0, x_54);
return x_55;
}
}
}
}
}
LEAN_EXPORT lean_object* l_MangoV4_perpSocializeLossTransition___boxed(lean_object* x_1, lean_object* x_2) {
_start:
{
lean_object* x_3; 
x_3 = l_MangoV4_perpSocializeLossTransition(x_1, x_2);
lean_dec(x_2);
return x_3;
}
}
LEAN_EXPORT lean_object* l_MangoV4_feeAccrualTransition(lean_object* x_1, lean_object* x_2, lean_object* x_3) {
_start:
{
uint8_t x_4; 
x_4 = !lean_is_exclusive(x_1);
if (x_4 == 0)
{
lean_object* x_5; lean_object* x_6; lean_object* x_7; 
x_5 = lean_ctor_get(x_1, 15);
x_6 = lean_int_add(x_5, x_3);
lean_dec(x_5);
x_7 = lean_int_add(x_6, x_2);
lean_dec(x_6);
lean_ctor_set(x_1, 15, x_7);
return x_1;
}
else
{
lean_object* x_8; lean_object* x_9; lean_object* x_10; lean_object* x_11; lean_object* x_12; lean_object* x_13; lean_object* x_14; lean_object* x_15; lean_object* x_16; lean_object* x_17; lean_object* x_18; lean_object* x_19; lean_object* x_20; lean_object* x_21; lean_object* x_22; lean_object* x_23; lean_object* x_24; lean_object* x_25; lean_object* x_26; lean_object* x_27; lean_object* x_28; lean_object* x_29; uint8_t x_30; uint8_t x_31; uint8_t x_32; lean_object* x_33; lean_object* x_34; lean_object* x_35; 
x_8 = lean_ctor_get(x_1, 0);
x_9 = lean_ctor_get(x_1, 1);
x_10 = lean_ctor_get(x_1, 2);
x_11 = lean_ctor_get(x_1, 3);
x_12 = lean_ctor_get(x_1, 4);
x_13 = lean_ctor_get(x_1, 5);
x_14 = lean_ctor_get(x_1, 6);
x_15 = lean_ctor_get(x_1, 7);
x_16 = lean_ctor_get(x_1, 8);
x_17 = lean_ctor_get(x_1, 9);
x_18 = lean_ctor_get(x_1, 10);
x_19 = lean_ctor_get(x_1, 11);
x_20 = lean_ctor_get(x_1, 12);
x_21 = lean_ctor_get(x_1, 13);
x_22 = lean_ctor_get(x_1, 14);
x_23 = lean_ctor_get(x_1, 15);
x_24 = lean_ctor_get(x_1, 16);
x_25 = lean_ctor_get(x_1, 17);
x_26 = lean_ctor_get(x_1, 18);
x_27 = lean_ctor_get(x_1, 19);
x_28 = lean_ctor_get(x_1, 20);
x_29 = lean_ctor_get(x_1, 21);
x_30 = lean_ctor_get_uint8(x_1, sizeof(void*)*22);
x_31 = lean_ctor_get_uint8(x_1, sizeof(void*)*22 + 1);
x_32 = lean_ctor_get_uint8(x_1, sizeof(void*)*22 + 2);
lean_inc(x_29);
lean_inc(x_28);
lean_inc(x_27);
lean_inc(x_26);
lean_inc(x_25);
lean_inc(x_24);
lean_inc(x_23);
lean_inc(x_22);
lean_inc(x_21);
lean_inc(x_20);
lean_inc(x_19);
lean_inc(x_18);
lean_inc(x_17);
lean_inc(x_16);
lean_inc(x_15);
lean_inc(x_14);
lean_inc(x_13);
lean_inc(x_12);
lean_inc(x_11);
lean_inc(x_10);
lean_inc(x_9);
lean_inc(x_8);
lean_dec(x_1);
x_33 = lean_int_add(x_23, x_3);
lean_dec(x_23);
x_34 = lean_int_add(x_33, x_2);
lean_dec(x_33);
x_35 = lean_alloc_ctor(0, 22, 3);
lean_ctor_set(x_35, 0, x_8);
lean_ctor_set(x_35, 1, x_9);
lean_ctor_set(x_35, 2, x_10);
lean_ctor_set(x_35, 3, x_11);
lean_ctor_set(x_35, 4, x_12);
lean_ctor_set(x_35, 5, x_13);
lean_ctor_set(x_35, 6, x_14);
lean_ctor_set(x_35, 7, x_15);
lean_ctor_set(x_35, 8, x_16);
lean_ctor_set(x_35, 9, x_17);
lean_ctor_set(x_35, 10, x_18);
lean_ctor_set(x_35, 11, x_19);
lean_ctor_set(x_35, 12, x_20);
lean_ctor_set(x_35, 13, x_21);
lean_ctor_set(x_35, 14, x_22);
lean_ctor_set(x_35, 15, x_34);
lean_ctor_set(x_35, 16, x_24);
lean_ctor_set(x_35, 17, x_25);
lean_ctor_set(x_35, 18, x_26);
lean_ctor_set(x_35, 19, x_27);
lean_ctor_set(x_35, 20, x_28);
lean_ctor_set(x_35, 21, x_29);
lean_ctor_set_uint8(x_35, sizeof(void*)*22, x_30);
lean_ctor_set_uint8(x_35, sizeof(void*)*22 + 1, x_31);
lean_ctor_set_uint8(x_35, sizeof(void*)*22 + 2, x_32);
return x_35;
}
}
}
LEAN_EXPORT lean_object* l_MangoV4_feeAccrualTransition___boxed(lean_object* x_1, lean_object* x_2, lean_object* x_3) {
_start:
{
lean_object* x_4; 
x_4 = l_MangoV4_feeAccrualTransition(x_1, x_2, x_3);
lean_dec(x_3);
lean_dec(x_2);
return x_4;
}
}
lean_object* initialize_Init(uint8_t builtin, lean_object*);
lean_object* initialize_MangoState_Types(uint8_t builtin, lean_object*);
lean_object* initialize_QEDGen_Solana(uint8_t builtin, lean_object*);
static bool _G_initialized = false;
LEAN_EXPORT lean_object* initialize_MangoState_PerpMarket(uint8_t builtin, lean_object* w) {
lean_object * res;
if (_G_initialized) return lean_io_result_mk_ok(lean_box(0));
_G_initialized = true;
res = initialize_Init(builtin, lean_io_mk_world());
if (lean_io_result_is_error(res)) return res;
lean_dec_ref(res);
res = initialize_MangoState_Types(builtin, lean_io_mk_world());
if (lean_io_result_is_error(res)) return res;
lean_dec_ref(res);
res = initialize_QEDGen_Solana(builtin, lean_io_mk_world());
if (lean_io_result_is_error(res)) return res;
lean_dec_ref(res);
l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__1 = _init_l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__1();
lean_mark_persistent(l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__1);
l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__2 = _init_l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__2();
lean_mark_persistent(l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__2);
l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__3 = _init_l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__3();
lean_mark_persistent(l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__3);
l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__4 = _init_l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__4();
lean_mark_persistent(l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__4);
l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__5 = _init_l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__5();
lean_mark_persistent(l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__5);
l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__6 = _init_l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__6();
lean_mark_persistent(l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__6);
l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__7 = _init_l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__7();
lean_mark_persistent(l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__7);
l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__8 = _init_l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__8();
lean_mark_persistent(l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__8);
l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__9 = _init_l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__9();
lean_mark_persistent(l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__9);
l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__10 = _init_l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__10();
lean_mark_persistent(l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__10);
l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__11 = _init_l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__11();
lean_mark_persistent(l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__11);
l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__12 = _init_l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__12();
lean_mark_persistent(l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__12);
l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__13 = _init_l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__13();
lean_mark_persistent(l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__13);
l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__14 = _init_l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__14();
lean_mark_persistent(l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__14);
l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__15 = _init_l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__15();
lean_mark_persistent(l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__15);
l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__16 = _init_l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__16();
lean_mark_persistent(l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__16);
l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__17 = _init_l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__17();
lean_mark_persistent(l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__17);
l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__18 = _init_l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__18();
lean_mark_persistent(l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__18);
l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__19 = _init_l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__19();
lean_mark_persistent(l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__19);
l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__20 = _init_l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__20();
lean_mark_persistent(l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__20);
l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__21 = _init_l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__21();
lean_mark_persistent(l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__21);
l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__22 = _init_l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__22();
lean_mark_persistent(l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__22);
l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__23 = _init_l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__23();
lean_mark_persistent(l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__23);
l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__24 = _init_l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__24();
lean_mark_persistent(l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__24);
l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__25 = _init_l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__25();
lean_mark_persistent(l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__25);
l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__26 = _init_l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__26();
lean_mark_persistent(l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__26);
l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__27 = _init_l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__27();
lean_mark_persistent(l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__27);
l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__28 = _init_l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__28();
lean_mark_persistent(l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__28);
l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__29 = _init_l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__29();
lean_mark_persistent(l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__29);
l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__30 = _init_l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__30();
lean_mark_persistent(l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__30);
l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__31 = _init_l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__31();
lean_mark_persistent(l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__31);
l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__32 = _init_l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__32();
lean_mark_persistent(l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__32);
l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__33 = _init_l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__33();
lean_mark_persistent(l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__33);
l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__34 = _init_l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__34();
lean_mark_persistent(l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__34);
l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__35 = _init_l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__35();
lean_mark_persistent(l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__35);
l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__36 = _init_l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__36();
lean_mark_persistent(l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__36);
l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__37 = _init_l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__37();
lean_mark_persistent(l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__37);
l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__38 = _init_l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__38();
lean_mark_persistent(l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__38);
l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__39 = _init_l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__39();
lean_mark_persistent(l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__39);
l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__40 = _init_l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__40();
lean_mark_persistent(l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__40);
l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__41 = _init_l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__41();
lean_mark_persistent(l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__41);
l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__42 = _init_l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__42();
lean_mark_persistent(l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__42);
l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__43 = _init_l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__43();
lean_mark_persistent(l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__43);
l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__44 = _init_l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__44();
lean_mark_persistent(l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__44);
l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__45 = _init_l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__45();
lean_mark_persistent(l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__45);
l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__46 = _init_l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__46();
lean_mark_persistent(l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__46);
l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__47 = _init_l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__47();
lean_mark_persistent(l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__47);
l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__48 = _init_l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__48();
lean_mark_persistent(l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__48);
l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__49 = _init_l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__49();
lean_mark_persistent(l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__49);
l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__50 = _init_l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__50();
lean_mark_persistent(l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__50);
l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__51 = _init_l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__51();
lean_mark_persistent(l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__51);
l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__52 = _init_l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__52();
lean_mark_persistent(l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__52);
l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__53 = _init_l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__53();
lean_mark_persistent(l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__53);
l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__54 = _init_l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__54();
lean_mark_persistent(l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__54);
l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__55 = _init_l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__55();
lean_mark_persistent(l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__55);
l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__56 = _init_l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__56();
lean_mark_persistent(l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__56);
l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__57 = _init_l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__57();
lean_mark_persistent(l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__57);
l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__58 = _init_l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__58();
lean_mark_persistent(l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__58);
l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__59 = _init_l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__59();
lean_mark_persistent(l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__59);
l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__60 = _init_l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__60();
lean_mark_persistent(l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__60);
l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__61 = _init_l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__61();
lean_mark_persistent(l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__61);
l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__62 = _init_l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__62();
lean_mark_persistent(l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__62);
l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__63 = _init_l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__63();
lean_mark_persistent(l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__63);
l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__64 = _init_l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__64();
lean_mark_persistent(l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__64);
l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__65 = _init_l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__65();
lean_mark_persistent(l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__65);
l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__66 = _init_l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__66();
lean_mark_persistent(l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__66);
l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__67 = _init_l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__67();
lean_mark_persistent(l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__67);
l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__68 = _init_l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__68();
lean_mark_persistent(l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__68);
l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__69 = _init_l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__69();
lean_mark_persistent(l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__69);
l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__70 = _init_l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__70();
lean_mark_persistent(l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__70);
l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__71 = _init_l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__71();
lean_mark_persistent(l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__71);
l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__72 = _init_l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__72();
lean_mark_persistent(l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__72);
l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__73 = _init_l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__73();
lean_mark_persistent(l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__73);
l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__74 = _init_l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__74();
lean_mark_persistent(l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__74);
l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__75 = _init_l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__75();
lean_mark_persistent(l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__75);
l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__76 = _init_l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__76();
lean_mark_persistent(l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__76);
l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__77 = _init_l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__77();
lean_mark_persistent(l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__77);
l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__78 = _init_l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__78();
lean_mark_persistent(l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__78);
l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__79 = _init_l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__79();
lean_mark_persistent(l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__79);
l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__80 = _init_l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__80();
lean_mark_persistent(l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__80);
l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__81 = _init_l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__81();
lean_mark_persistent(l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__81);
l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__82 = _init_l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__82();
lean_mark_persistent(l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__82);
l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__83 = _init_l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__83();
lean_mark_persistent(l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__83);
l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__84 = _init_l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__84();
lean_mark_persistent(l___private_MangoState_PerpMarket_0__MangoV4_reprPerpMarket____x40_MangoState_PerpMarket___hyg_210____closed__84);
l_MangoV4_instReprPerpMarket___closed__1 = _init_l_MangoV4_instReprPerpMarket___closed__1();
lean_mark_persistent(l_MangoV4_instReprPerpMarket___closed__1);
l_MangoV4_instReprPerpMarket = _init_l_MangoV4_instReprPerpMarket();
lean_mark_persistent(l_MangoV4_instReprPerpMarket);
l___private_MangoState_PerpMarket_0__MangoV4_reprPerpPosition____x40_MangoState_PerpMarket___hyg_829____closed__1 = _init_l___private_MangoState_PerpMarket_0__MangoV4_reprPerpPosition____x40_MangoState_PerpMarket___hyg_829____closed__1();
lean_mark_persistent(l___private_MangoState_PerpMarket_0__MangoV4_reprPerpPosition____x40_MangoState_PerpMarket___hyg_829____closed__1);
l___private_MangoState_PerpMarket_0__MangoV4_reprPerpPosition____x40_MangoState_PerpMarket___hyg_829____closed__2 = _init_l___private_MangoState_PerpMarket_0__MangoV4_reprPerpPosition____x40_MangoState_PerpMarket___hyg_829____closed__2();
lean_mark_persistent(l___private_MangoState_PerpMarket_0__MangoV4_reprPerpPosition____x40_MangoState_PerpMarket___hyg_829____closed__2);
l___private_MangoState_PerpMarket_0__MangoV4_reprPerpPosition____x40_MangoState_PerpMarket___hyg_829____closed__3 = _init_l___private_MangoState_PerpMarket_0__MangoV4_reprPerpPosition____x40_MangoState_PerpMarket___hyg_829____closed__3();
lean_mark_persistent(l___private_MangoState_PerpMarket_0__MangoV4_reprPerpPosition____x40_MangoState_PerpMarket___hyg_829____closed__3);
l___private_MangoState_PerpMarket_0__MangoV4_reprPerpPosition____x40_MangoState_PerpMarket___hyg_829____closed__4 = _init_l___private_MangoState_PerpMarket_0__MangoV4_reprPerpPosition____x40_MangoState_PerpMarket___hyg_829____closed__4();
lean_mark_persistent(l___private_MangoState_PerpMarket_0__MangoV4_reprPerpPosition____x40_MangoState_PerpMarket___hyg_829____closed__4);
l___private_MangoState_PerpMarket_0__MangoV4_reprPerpPosition____x40_MangoState_PerpMarket___hyg_829____closed__5 = _init_l___private_MangoState_PerpMarket_0__MangoV4_reprPerpPosition____x40_MangoState_PerpMarket___hyg_829____closed__5();
lean_mark_persistent(l___private_MangoState_PerpMarket_0__MangoV4_reprPerpPosition____x40_MangoState_PerpMarket___hyg_829____closed__5);
l___private_MangoState_PerpMarket_0__MangoV4_reprPerpPosition____x40_MangoState_PerpMarket___hyg_829____closed__6 = _init_l___private_MangoState_PerpMarket_0__MangoV4_reprPerpPosition____x40_MangoState_PerpMarket___hyg_829____closed__6();
lean_mark_persistent(l___private_MangoState_PerpMarket_0__MangoV4_reprPerpPosition____x40_MangoState_PerpMarket___hyg_829____closed__6);
l___private_MangoState_PerpMarket_0__MangoV4_reprPerpPosition____x40_MangoState_PerpMarket___hyg_829____closed__7 = _init_l___private_MangoState_PerpMarket_0__MangoV4_reprPerpPosition____x40_MangoState_PerpMarket___hyg_829____closed__7();
lean_mark_persistent(l___private_MangoState_PerpMarket_0__MangoV4_reprPerpPosition____x40_MangoState_PerpMarket___hyg_829____closed__7);
l___private_MangoState_PerpMarket_0__MangoV4_reprPerpPosition____x40_MangoState_PerpMarket___hyg_829____closed__8 = _init_l___private_MangoState_PerpMarket_0__MangoV4_reprPerpPosition____x40_MangoState_PerpMarket___hyg_829____closed__8();
lean_mark_persistent(l___private_MangoState_PerpMarket_0__MangoV4_reprPerpPosition____x40_MangoState_PerpMarket___hyg_829____closed__8);
l___private_MangoState_PerpMarket_0__MangoV4_reprPerpPosition____x40_MangoState_PerpMarket___hyg_829____closed__9 = _init_l___private_MangoState_PerpMarket_0__MangoV4_reprPerpPosition____x40_MangoState_PerpMarket___hyg_829____closed__9();
lean_mark_persistent(l___private_MangoState_PerpMarket_0__MangoV4_reprPerpPosition____x40_MangoState_PerpMarket___hyg_829____closed__9);
l___private_MangoState_PerpMarket_0__MangoV4_reprPerpPosition____x40_MangoState_PerpMarket___hyg_829____closed__10 = _init_l___private_MangoState_PerpMarket_0__MangoV4_reprPerpPosition____x40_MangoState_PerpMarket___hyg_829____closed__10();
lean_mark_persistent(l___private_MangoState_PerpMarket_0__MangoV4_reprPerpPosition____x40_MangoState_PerpMarket___hyg_829____closed__10);
l___private_MangoState_PerpMarket_0__MangoV4_reprPerpPosition____x40_MangoState_PerpMarket___hyg_829____closed__11 = _init_l___private_MangoState_PerpMarket_0__MangoV4_reprPerpPosition____x40_MangoState_PerpMarket___hyg_829____closed__11();
lean_mark_persistent(l___private_MangoState_PerpMarket_0__MangoV4_reprPerpPosition____x40_MangoState_PerpMarket___hyg_829____closed__11);
l___private_MangoState_PerpMarket_0__MangoV4_reprPerpPosition____x40_MangoState_PerpMarket___hyg_829____closed__12 = _init_l___private_MangoState_PerpMarket_0__MangoV4_reprPerpPosition____x40_MangoState_PerpMarket___hyg_829____closed__12();
lean_mark_persistent(l___private_MangoState_PerpMarket_0__MangoV4_reprPerpPosition____x40_MangoState_PerpMarket___hyg_829____closed__12);
l___private_MangoState_PerpMarket_0__MangoV4_reprPerpPosition____x40_MangoState_PerpMarket___hyg_829____closed__13 = _init_l___private_MangoState_PerpMarket_0__MangoV4_reprPerpPosition____x40_MangoState_PerpMarket___hyg_829____closed__13();
lean_mark_persistent(l___private_MangoState_PerpMarket_0__MangoV4_reprPerpPosition____x40_MangoState_PerpMarket___hyg_829____closed__13);
l___private_MangoState_PerpMarket_0__MangoV4_reprPerpPosition____x40_MangoState_PerpMarket___hyg_829____closed__14 = _init_l___private_MangoState_PerpMarket_0__MangoV4_reprPerpPosition____x40_MangoState_PerpMarket___hyg_829____closed__14();
lean_mark_persistent(l___private_MangoState_PerpMarket_0__MangoV4_reprPerpPosition____x40_MangoState_PerpMarket___hyg_829____closed__14);
l___private_MangoState_PerpMarket_0__MangoV4_reprPerpPosition____x40_MangoState_PerpMarket___hyg_829____closed__15 = _init_l___private_MangoState_PerpMarket_0__MangoV4_reprPerpPosition____x40_MangoState_PerpMarket___hyg_829____closed__15();
lean_mark_persistent(l___private_MangoState_PerpMarket_0__MangoV4_reprPerpPosition____x40_MangoState_PerpMarket___hyg_829____closed__15);
l___private_MangoState_PerpMarket_0__MangoV4_reprPerpPosition____x40_MangoState_PerpMarket___hyg_829____closed__16 = _init_l___private_MangoState_PerpMarket_0__MangoV4_reprPerpPosition____x40_MangoState_PerpMarket___hyg_829____closed__16();
lean_mark_persistent(l___private_MangoState_PerpMarket_0__MangoV4_reprPerpPosition____x40_MangoState_PerpMarket___hyg_829____closed__16);
l___private_MangoState_PerpMarket_0__MangoV4_reprPerpPosition____x40_MangoState_PerpMarket___hyg_829____closed__17 = _init_l___private_MangoState_PerpMarket_0__MangoV4_reprPerpPosition____x40_MangoState_PerpMarket___hyg_829____closed__17();
lean_mark_persistent(l___private_MangoState_PerpMarket_0__MangoV4_reprPerpPosition____x40_MangoState_PerpMarket___hyg_829____closed__17);
l___private_MangoState_PerpMarket_0__MangoV4_reprPerpPosition____x40_MangoState_PerpMarket___hyg_829____closed__18 = _init_l___private_MangoState_PerpMarket_0__MangoV4_reprPerpPosition____x40_MangoState_PerpMarket___hyg_829____closed__18();
lean_mark_persistent(l___private_MangoState_PerpMarket_0__MangoV4_reprPerpPosition____x40_MangoState_PerpMarket___hyg_829____closed__18);
l___private_MangoState_PerpMarket_0__MangoV4_reprPerpPosition____x40_MangoState_PerpMarket___hyg_829____closed__19 = _init_l___private_MangoState_PerpMarket_0__MangoV4_reprPerpPosition____x40_MangoState_PerpMarket___hyg_829____closed__19();
lean_mark_persistent(l___private_MangoState_PerpMarket_0__MangoV4_reprPerpPosition____x40_MangoState_PerpMarket___hyg_829____closed__19);
l___private_MangoState_PerpMarket_0__MangoV4_reprPerpPosition____x40_MangoState_PerpMarket___hyg_829____closed__20 = _init_l___private_MangoState_PerpMarket_0__MangoV4_reprPerpPosition____x40_MangoState_PerpMarket___hyg_829____closed__20();
lean_mark_persistent(l___private_MangoState_PerpMarket_0__MangoV4_reprPerpPosition____x40_MangoState_PerpMarket___hyg_829____closed__20);
l___private_MangoState_PerpMarket_0__MangoV4_reprPerpPosition____x40_MangoState_PerpMarket___hyg_829____closed__21 = _init_l___private_MangoState_PerpMarket_0__MangoV4_reprPerpPosition____x40_MangoState_PerpMarket___hyg_829____closed__21();
lean_mark_persistent(l___private_MangoState_PerpMarket_0__MangoV4_reprPerpPosition____x40_MangoState_PerpMarket___hyg_829____closed__21);
l___private_MangoState_PerpMarket_0__MangoV4_reprPerpPosition____x40_MangoState_PerpMarket___hyg_829____closed__22 = _init_l___private_MangoState_PerpMarket_0__MangoV4_reprPerpPosition____x40_MangoState_PerpMarket___hyg_829____closed__22();
lean_mark_persistent(l___private_MangoState_PerpMarket_0__MangoV4_reprPerpPosition____x40_MangoState_PerpMarket___hyg_829____closed__22);
l___private_MangoState_PerpMarket_0__MangoV4_reprPerpPosition____x40_MangoState_PerpMarket___hyg_829____closed__23 = _init_l___private_MangoState_PerpMarket_0__MangoV4_reprPerpPosition____x40_MangoState_PerpMarket___hyg_829____closed__23();
lean_mark_persistent(l___private_MangoState_PerpMarket_0__MangoV4_reprPerpPosition____x40_MangoState_PerpMarket___hyg_829____closed__23);
l___private_MangoState_PerpMarket_0__MangoV4_reprPerpPosition____x40_MangoState_PerpMarket___hyg_829____closed__24 = _init_l___private_MangoState_PerpMarket_0__MangoV4_reprPerpPosition____x40_MangoState_PerpMarket___hyg_829____closed__24();
lean_mark_persistent(l___private_MangoState_PerpMarket_0__MangoV4_reprPerpPosition____x40_MangoState_PerpMarket___hyg_829____closed__24);
l___private_MangoState_PerpMarket_0__MangoV4_reprPerpPosition____x40_MangoState_PerpMarket___hyg_829____closed__25 = _init_l___private_MangoState_PerpMarket_0__MangoV4_reprPerpPosition____x40_MangoState_PerpMarket___hyg_829____closed__25();
lean_mark_persistent(l___private_MangoState_PerpMarket_0__MangoV4_reprPerpPosition____x40_MangoState_PerpMarket___hyg_829____closed__25);
l___private_MangoState_PerpMarket_0__MangoV4_reprPerpPosition____x40_MangoState_PerpMarket___hyg_829____closed__26 = _init_l___private_MangoState_PerpMarket_0__MangoV4_reprPerpPosition____x40_MangoState_PerpMarket___hyg_829____closed__26();
lean_mark_persistent(l___private_MangoState_PerpMarket_0__MangoV4_reprPerpPosition____x40_MangoState_PerpMarket___hyg_829____closed__26);
l___private_MangoState_PerpMarket_0__MangoV4_reprPerpPosition____x40_MangoState_PerpMarket___hyg_829____closed__27 = _init_l___private_MangoState_PerpMarket_0__MangoV4_reprPerpPosition____x40_MangoState_PerpMarket___hyg_829____closed__27();
lean_mark_persistent(l___private_MangoState_PerpMarket_0__MangoV4_reprPerpPosition____x40_MangoState_PerpMarket___hyg_829____closed__27);
l___private_MangoState_PerpMarket_0__MangoV4_reprPerpPosition____x40_MangoState_PerpMarket___hyg_829____closed__28 = _init_l___private_MangoState_PerpMarket_0__MangoV4_reprPerpPosition____x40_MangoState_PerpMarket___hyg_829____closed__28();
lean_mark_persistent(l___private_MangoState_PerpMarket_0__MangoV4_reprPerpPosition____x40_MangoState_PerpMarket___hyg_829____closed__28);
l___private_MangoState_PerpMarket_0__MangoV4_reprPerpPosition____x40_MangoState_PerpMarket___hyg_829____closed__29 = _init_l___private_MangoState_PerpMarket_0__MangoV4_reprPerpPosition____x40_MangoState_PerpMarket___hyg_829____closed__29();
lean_mark_persistent(l___private_MangoState_PerpMarket_0__MangoV4_reprPerpPosition____x40_MangoState_PerpMarket___hyg_829____closed__29);
l___private_MangoState_PerpMarket_0__MangoV4_reprPerpPosition____x40_MangoState_PerpMarket___hyg_829____closed__30 = _init_l___private_MangoState_PerpMarket_0__MangoV4_reprPerpPosition____x40_MangoState_PerpMarket___hyg_829____closed__30();
lean_mark_persistent(l___private_MangoState_PerpMarket_0__MangoV4_reprPerpPosition____x40_MangoState_PerpMarket___hyg_829____closed__30);
l___private_MangoState_PerpMarket_0__MangoV4_reprPerpPosition____x40_MangoState_PerpMarket___hyg_829____closed__31 = _init_l___private_MangoState_PerpMarket_0__MangoV4_reprPerpPosition____x40_MangoState_PerpMarket___hyg_829____closed__31();
lean_mark_persistent(l___private_MangoState_PerpMarket_0__MangoV4_reprPerpPosition____x40_MangoState_PerpMarket___hyg_829____closed__31);
l___private_MangoState_PerpMarket_0__MangoV4_reprPerpPosition____x40_MangoState_PerpMarket___hyg_829____closed__32 = _init_l___private_MangoState_PerpMarket_0__MangoV4_reprPerpPosition____x40_MangoState_PerpMarket___hyg_829____closed__32();
lean_mark_persistent(l___private_MangoState_PerpMarket_0__MangoV4_reprPerpPosition____x40_MangoState_PerpMarket___hyg_829____closed__32);
l_MangoV4_instReprPerpPosition___closed__1 = _init_l_MangoV4_instReprPerpPosition___closed__1();
lean_mark_persistent(l_MangoV4_instReprPerpPosition___closed__1);
l_MangoV4_instReprPerpPosition = _init_l_MangoV4_instReprPerpPosition();
lean_mark_persistent(l_MangoV4_instReprPerpPosition);
return lean_io_result_mk_ok(lean_box(0));
}
#ifdef __cplusplus
}
#endif
