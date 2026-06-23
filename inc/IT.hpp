/*-------------------------------------------------------------------------------
 *\file IT.hpp
 *\info Header file for the interpreter
 * *----------------------------------------------------------------------------*/

#ifndef IT_HEADER_
#define IT_HEADER_

/*------------------------------------------------------------------------------
 *\INCLUDES
 *-----------------------------------------------------------------------------*/
#include "EX.hpp"
#include "ITxDATA.hpp"

namespace IT
{

pLm exprs2pLm(EX::Expr *expr, StatEnv &env);

std::string pprint(pLm lm, int level = 0);

pLm eval(pLm node, DynEnv denv, StatEnv &senv);

} // namespace IT

#endif // IT_HEADER_

/*-------------------------------------------------------------------------------
 *\EOF
 *------------------------------------------------------------------------------*/
